use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use crate::{
    ghostty::{
        self, MODE_BRACKETED_PASTE, MODE_MOUSE_ALTERNATE_SCROLL, MODE_MOUSE_SGR, MODE_MOUSE_UTF8,
    },
    ui::{agent_binary_for_command, agent_command_for_input, pane_chrome_title_label},
    utils::{resolve_login_shell_command, LOGIN_SHELL_SENTINEL},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use nix::{
    pty::{forkpty, ForkptyResult, Winsize},
    sys::wait::{waitpid, WaitPidFlag},
    unistd::{dup, execvp, Pid},
};
use ratatui::{layout::Rect, Frame};

/// Ghostty scrollback line capacity (Herdr-style harness-owned history).
const SCROLLBACK_LINES: usize = 50_000;

/// Mouse wheel scroll step (Herdr default is 3 lines per notch).
pub(crate) const MOUSE_SCROLL_LINES: usize = 3;

const PANE_TERM: &str = "xterm-256color";
const PANE_COLORTERM: &str = "truecolor";

const MODE_MOUSE_X10: u16 = 9;
const MODE_MOUSE_PRESS_RELEASE: u16 = 1000;
const MODE_MOUSE_BUTTON_MOTION: u16 = 1002;
const MODE_MOUSE_ANY_MOTION: u16 = 1003;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScrollMetrics {
    pub viewport_offset: usize,
    pub offset_from_bottom: usize,
    pub max_offset_from_bottom: usize,
    pub viewport_rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionAnchor {
    pub col: u16,
    pub screen_row: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseProtocolMode {
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseProtocolEncoding {
    Default,
    Utf8,
    Sgr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InputState {
    bracketed_paste: bool,
    mouse_protocol_mode: MouseProtocolMode,
    mouse_protocol_encoding: MouseProtocolEncoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaneMouseEventKind {
    Down,
    Up,
    Drag,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneSelection {
    pub(crate) start: (u16, u16),
    pub(crate) end: (u16, u16),
}

pub(crate) struct Pane {
    pub(crate) id: usize,
    pub(crate) title: String,
    pub(crate) command: String,
    pub(crate) resume_command: Option<String>,
    pub(crate) last_command: Option<String>,
    agent_binary: Option<&'static str>,
    pub(crate) exited: bool,
    pub(crate) relaunch_failed: bool,
    terminal: ghostty::Terminal,
    render_state: ghostty::RenderState,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial_default_foreground: Option<ghostty::RgbColor>,
    initial_default_background: Option<ghostty::RgbColor>,
    writer: File,
    rx: Receiver<Vec<u8>>,
    child: Pid,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    last_replayed_command: Option<String>,
    input_buffer: String,
    input_cursor: usize,
    /// Commander panels are UI-only and do not own a PTY child.
    stub: bool,
}

impl Drop for Pane {
    fn drop(&mut self) {
        if self.stub {
            return;
        }
        unsafe {
            libc::kill(self.child.as_raw(), libc::SIGTERM);
        }
        let _ = waitpid(self.child, Some(WaitPidFlag::WNOHANG));
    }
}

impl Pane {
    pub(crate) fn new_commander(id: usize, rows: u16, cols: u16) -> Self {
        let (_tx, rx) = mpsc::channel();
        let (terminal, render_state, initial_default_foreground, initial_default_background) =
            make_ghostty_state(rows.max(1), cols.max(1)).unwrap_or_else(|_| {
                let terminal = ghostty::Terminal::new(1, 1, 0).expect("commander terminal");
                let render_state = ghostty::RenderState::new().expect("commander render state");
                (terminal, render_state, None, None)
            });
        Self {
            id,
            title: "Commander".to_string(),
            command: "commander".to_string(),
            resume_command: None,
            last_command: None,
            agent_binary: None,
            exited: false,
            relaunch_failed: false,
            terminal,
            render_state,
            host_theme: crate::terminal_theme::TerminalTheme::default(),
            initial_default_foreground,
            initial_default_background,
            writer: File::open("/dev/null").unwrap_or_else(|_| {
                File::create("/dev/null").expect("open /dev/null for commander stub pane")
            }),
            rx,
            child: Pid::from_raw(0),
            cols,
            rows,
            last_replayed_command: None,
            input_buffer: String::new(),
            input_cursor: 0,
            stub: true,
        }
    }

    pub(crate) fn is_stub(&self) -> bool {
        self.stub
    }

    pub(crate) fn chrome_title_label(&self) -> String {
        pane_chrome_title_label(&self.title, &self.command)
    }

    pub(crate) fn new(
        id: usize,
        title: impl Into<String>,
        command: impl Into<String>,
        resume_command: Option<String>,
        last_command: Option<String>,
        rows: u16,
        cols: u16,
        initial_scroll_offset: Option<usize>,
    ) -> anyhow::Result<Self> {
        let command: String = command.into();
        let pane_command = match resume_command.as_deref() {
            Some(line) => line.to_string(),
            None => resolve_login_shell_command(&command),
        };
        let agent_binary = agent_binary_for_command(&command);
        let rows = rows.max(1);
        let cols = cols.max(1);
        let ws = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let fork_result = unsafe { forkpty(Some(&ws), None)? };

        match fork_result {
            ForkptyResult::Child => {
                set_child_terminal_env();
                exec_command(&pane_command);
            }
            ForkptyResult::Parent { child, master } => {
                let reader_fd = unsafe { OwnedFd::from_raw_fd(dup(master.as_raw_fd())?) };
                let writer = File::from(master);
                let reader = File::from(reader_fd);
                let (tx, rx) = mpsc::channel();

                thread::spawn(move || pump_pty_output(reader, tx));

                let (
                    terminal,
                    render_state,
                    initial_default_foreground,
                    initial_default_background,
                ) = make_ghostty_state(rows, cols)?;
                resize_pty(writer.as_raw_fd(), rows, cols)?;
                let mut pane = Self {
                    id,
                    title: title.into(),
                    command,
                    resume_command,
                    last_command,
                    agent_binary,
                    exited: false,
                    relaunch_failed: false,
                    terminal,
                    render_state,
                    host_theme: crate::terminal_theme::TerminalTheme::default(),
                    initial_default_foreground,
                    initial_default_background,
                    writer,
                    rx,
                    child,
                    cols,
                    rows,
                    last_replayed_command: None,
                    input_buffer: String::new(),
                    input_cursor: 0,
                    stub: false,
                };
                if let Some(offset) = initial_scroll_offset.filter(|offset| *offset > 0) {
                    pane.set_scroll_offset_from_bottom(offset);
                }
                let _ = pane.render_state.update(&pane.terminal);
                Ok(pane)
            }
        }
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        if self.stub {
            self.rows = rows.max(1);
            self.cols = cols.max(1);
            return;
        }
        if rows == self.rows && cols == self.cols {
            return;
        }

        let offset_from_bottom = self
            .scroll_metrics()
            .map(|metrics| metrics.offset_from_bottom)
            .unwrap_or(0);
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        let _ = resize_pty(self.writer.as_raw_fd(), self.rows, self.cols);
        let _ = self.terminal.resize(self.cols, self.rows, 8, 16);
        let _ = self.render_state.update(&self.terminal);
        self.set_scroll_offset_from_bottom(offset_from_bottom);
    }

    pub(crate) fn pump(&mut self) -> bool {
        if self.stub {
            return false;
        }
        let mut processed = false;
        let mut disconnected = false;
        loop {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    self.terminal.write(&bytes);
                    let _ = self.render_state.update(&self.terminal);
                    processed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected && !self.exited {
            self.exited = true;
            self.capture_resume_command();
            processed = true;
        }
        processed
    }

    pub(crate) fn mark_painted(&mut self) {}

    pub(crate) fn terminate_session(&self) {}

    pub(crate) fn relaunch_as_shell(&mut self) -> anyhow::Result<()> {
        if self.stub {
            return Ok(());
        }
        let command = self.command.clone();
        let resume_command = self.resume_command.clone();
        let last_command = self.last_command.clone();
        let last_replayed_command = self.last_replayed_command.clone();
        let agent_binary = self.agent_binary;

        let mut new = Pane::new(
            self.id,
            self.title.clone(),
            LOGIN_SHELL_SENTINEL,
            None,
            None,
            self.rows,
            self.cols,
            None,
        )?;
        new.command = command;
        new.resume_command = resume_command;
        new.last_command = last_command;
        new.last_replayed_command = last_replayed_command;
        new.agent_binary = agent_binary;
        let _ = new.replay_last_command();
        *self = new;
        Ok(())
    }

    pub(crate) fn launch_agent_command(&mut self, command: &str) -> anyhow::Result<()> {
        let command = command.trim();
        if command.is_empty() {
            return Ok(());
        }
        self.send_paste(command)?;
        self.send(&[b'\r'])?;
        self.last_command = Some(command.to_string());
        self.last_replayed_command = Some(command.to_string());
        self.set_command(command.to_string());
        Ok(())
    }

    pub(crate) fn change_directory(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        self.change_directory_internal(path, false)
    }

    pub(crate) fn change_directory_for_open(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        self.change_directory_internal(path, true)
    }

    fn change_directory_internal(
        &mut self,
        path: &std::path::Path,
        clear: bool,
    ) -> anyhow::Result<()> {
        let quoted = shell_quote(&path.to_string_lossy());
        let cmd = format!("cd {quoted}");
        self.send(cmd.as_bytes())?;
        self.send(&[b'\r'])?;
        if clear {
            self.send(b"clear")?;
            self.send(&[b'\r'])?;
        }
        Ok(())
    }

    pub(crate) fn tmux_pane_path(&self) -> Option<PathBuf> {
        if self.stub {
            return None;
        }
        pane_cwd_from_pid(self.child.as_raw())
    }

    pub(crate) fn persisted_scroll_offset(&self) -> usize {
        self.scroll_metrics()
            .map(|metrics| metrics.offset_from_bottom)
            .unwrap_or(0)
    }

    pub(crate) fn scrolled_up(&self) -> bool {
        self.persisted_scroll_offset() > 0
    }

    pub(crate) fn scroll_lines_above_bottom(&self) -> usize {
        self.persisted_scroll_offset()
    }

    pub(crate) fn try_capture_resume_command(&mut self) {
        self.capture_resume_command();
    }

    fn capture_resume_command(&mut self) {
        let Some(binary) = self.agent_binary else {
            return;
        };
        let saved = self.scroll_metrics().map(|m| m.offset_from_bottom).unwrap_or(0);
        self.terminal.scroll_viewport_bottom();
        let rows = self.viewport_plain_rows();
        if saved > 0 {
            self.set_scroll_offset_from_bottom(saved);
        }
        if let Some(line) = extract_resume_command(binary, &rows) {
            self.resume_command = Some(line);
        }
    }

    pub(crate) fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if self.stub {
            return Ok(());
        }
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub(crate) fn refresh_terminal(&mut self) -> anyhow::Result<()> {
        if self.stub {
            return Ok(());
        }
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use crate::utils::key_to_bytes;

        const KEY_DELAY: Duration = Duration::from_millis(100);
        let keys = [
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        ];
        for (index, key) in keys.into_iter().enumerate() {
            if index > 0 {
                thread::sleep(KEY_DELAY);
            }
            self.send(&key_to_bytes(key))?;
        }
        Ok(())
    }

    pub(crate) fn send_paste(&mut self, text: &str) -> anyhow::Result<()> {
        if self.input_state().map(|s| s.bracketed_paste).unwrap_or(false) {
            self.writer.write_all(b"\x1b[200~")?;
            self.writer.write_all(text.as_bytes())?;
            self.writer.write_all(b"\x1b[201~")?;
            self.writer.flush()?;
            Ok(())
        } else {
            self.send(text.as_bytes())
        }
    }

    pub(crate) fn send_mouse_wheel(&mut self, up: bool, x: u16, y: u16) -> anyhow::Result<bool> {
        let Some(state) = self.input_state() else {
            return Ok(false);
        };
        if state.mouse_protocol_mode == MouseProtocolMode::None {
            return Ok(false);
        }

        let button = if up { 64 } else { 65 };
        let x = x.min(self.cols.saturating_sub(1)).saturating_add(1);
        let y = y.min(self.rows.saturating_sub(1)).saturating_add(1);
        let bytes = match state.mouse_protocol_encoding {
            MouseProtocolEncoding::Sgr => format!("\x1b[<{};{};{}M", button, x, y).into_bytes(),
            MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
                if x > 223 || y > 223 {
                    return Ok(false);
                }
                vec![
                    0x1b,
                    b'[',
                    b'M',
                    (32 + button) as u8,
                    (32 + x) as u8,
                    (32 + y) as u8,
                ]
            }
        };

        self.send(&bytes)?;
        Ok(true)
    }

    pub(crate) fn send_mouse_button(
        &mut self,
        button: MouseButton,
        event_kind: PaneMouseEventKind,
        modifiers: KeyModifiers,
        x: u16,
        y: u16,
    ) -> anyhow::Result<bool> {
        let Some(state) = self.input_state() else {
            return Ok(false);
        };
        let mode = state.mouse_protocol_mode;
        if mode == MouseProtocolMode::None {
            return Ok(false);
        }

        if matches!(event_kind, PaneMouseEventKind::Up)
            && !matches!(
                mode,
                MouseProtocolMode::PressRelease
                    | MouseProtocolMode::ButtonMotion
                    | MouseProtocolMode::AnyMotion
            )
        {
            return Ok(false);
        }

        if matches!(event_kind, PaneMouseEventKind::Drag)
            && !matches!(mode, MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion)
        {
            return Ok(false);
        }

        let button_code = match button {
            MouseButton::Left => 0u16,
            MouseButton::Middle => 1u16,
            MouseButton::Right => 2u16,
        };
        let modifier_bits = mouse_modifier_bits(modifiers);

        let code = match event_kind {
            PaneMouseEventKind::Down => button_code + modifier_bits,
            PaneMouseEventKind::Drag => button_code + modifier_bits + 32,
            PaneMouseEventKind::Up => match state.mouse_protocol_encoding {
                MouseProtocolEncoding::Sgr => button_code + modifier_bits,
                MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
                    3 + modifier_bits
                }
            },
        };

        let x = x.min(self.cols.saturating_sub(1)).saturating_add(1);
        let y = y.min(self.rows.saturating_sub(1)).saturating_add(1);

        let bytes = match state.mouse_protocol_encoding {
            MouseProtocolEncoding::Sgr => {
                let suffix = if matches!(event_kind, PaneMouseEventKind::Up) {
                    'm'
                } else {
                    'M'
                };
                format!("\x1b[<{};{};{}{}", code, x, y, suffix).into_bytes()
            }
            MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
                if x > 223 || y > 223 {
                    return Ok(false);
                }
                vec![
                    0x1b,
                    b'[',
                    b'M',
                    (32 + code) as u8,
                    (32 + x) as u8,
                    (32 + y) as u8,
                ]
            }
        };

        self.send(&bytes)?;
        Ok(true)
    }

    pub(crate) fn track_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.finalize_tracked_command(),
            KeyCode::Tab => self.insert_tracked_text("\t"),
            KeyCode::Left => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.input_cursor = (self.input_cursor + 1).min(self.input_buffer.chars().count());
            }
            KeyCode::Home => {
                self.input_cursor = 0;
            }
            KeyCode::End => {
                self.input_cursor = self.input_buffer.chars().count();
            }
            KeyCode::Backspace => {
                self.remove_tracked_char_before_cursor();
            }
            KeyCode::Delete => {
                self.remove_tracked_char_at_cursor();
            }
            KeyCode::Up | KeyCode::Down => {
                self.clear_pending_input();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let mut buf = [0u8; 4];
                let text = ch.encode_utf8(&mut buf);
                self.insert_tracked_text(text);
            }
            _ => {}
        }
    }

    pub(crate) fn track_paste(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\r' || ch == '\n' {
                self.finalize_tracked_command();
            } else {
                let mut buf = [0u8; 4];
                let text = ch.encode_utf8(&mut buf);
                self.insert_tracked_text(text);
            }
        }
    }

    pub(crate) fn clear_pending_input(&mut self) {
        self.input_buffer.clear();
        self.input_cursor = 0;
    }

    pub(crate) fn replay_last_command(&mut self) -> anyhow::Result<bool> {
        let Some(command) = self
            .last_command
            .as_deref()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
        else {
            return Ok(false);
        };

        if self.last_replayed_command.as_deref() == Some(command.as_str()) {
            return Ok(false);
        }

        self.send_paste(&command)?;
        self.send(&[b'\r'])?;
        self.last_replayed_command = Some(command);
        Ok(true)
    }

    pub(crate) fn set_command(&mut self, command: String) {
        self.command = command;
        self.agent_binary = agent_binary_for_command(&self.command);
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) -> bool {
        let before = self.scroll_metrics().map(|m| m.offset_from_bottom);
        self.terminal.scroll_viewport_delta(delta);
        let after = self.scroll_metrics().map(|m| m.offset_from_bottom);
        let changed = before != after;
        if changed {
            let _ = self.render_state.update(&self.terminal);
        }
        changed
    }

    pub(crate) fn scroll_up(&mut self, lines: usize) -> bool {
        self.scroll_by(-(lines.max(1) as isize))
    }

    pub(crate) fn scroll_down(&mut self, lines: usize) -> bool {
        self.scroll_by(lines.max(1) as isize)
    }

    pub(crate) fn page_up(&mut self) -> bool {
        self.scroll_up(self.rows.max(1) as usize)
    }

    pub(crate) fn page_down(&mut self) -> bool {
        self.scroll_down(self.rows.max(1) as usize)
    }

    pub(crate) fn scroll_top(&mut self) -> bool {
        let before = self.scroll_metrics().map(|m| m.offset_from_bottom);
        self.terminal.scroll_viewport_top();
        let after = self.scroll_metrics().map(|m| m.offset_from_bottom);
        let changed = before != after;
        if changed {
            let _ = self.render_state.update(&self.terminal);
        }
        changed
    }

    pub(crate) fn scroll_bottom(&mut self) -> bool {
        let before = self.scroll_metrics().map(|m| m.offset_from_bottom);
        self.terminal.scroll_viewport_bottom();
        let after = self.scroll_metrics().map(|m| m.offset_from_bottom);
        let changed = before != after;
        if changed {
            let _ = self.render_state.update(&self.terminal);
        }
        changed
    }

    pub(crate) fn tmux_copy_mode_active(&self) -> bool {
        false
    }

    pub(crate) fn leave_tmux_copy_mode(&self) {}

    pub(crate) fn render_terminal(&mut self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        if self.stub || area.width == 0 || area.height == 0 {
            return;
        }
        let show_cursor = show_cursor && !self.scrolled_up();
        crate::ghostty_render::render_pane(
            &self.terminal,
            &mut self.render_state,
            self.host_theme,
            self.initial_default_foreground,
            self.initial_default_background,
            frame,
            area,
            show_cursor,
        );
    }

    pub(crate) fn selected_text(&self, selection: PaneSelection) -> String {
        let ((start_col, start_row), (end_col, end_row)) = normalized_selection(selection);
        self.terminal
            .read_text_viewport(
                (start_col, u32::from(start_row)),
                (end_col, u32::from(end_row)),
                false,
            )
            .unwrap_or_default()
    }

    pub(crate) fn selection_anchor_from_viewport(&self, col: u16, viewport_row: u16) -> Option<SelectionAnchor> {
        let metrics = self.scroll_metrics()?;
        Some(SelectionAnchor {
            col,
            screen_row: metrics.viewport_offset as u32 + u32::from(viewport_row),
        })
    }

    pub(crate) fn viewport_coords_from_anchor(&self, anchor: SelectionAnchor) -> Option<(u16, u16)> {
        let metrics = self.scroll_metrics()?;
        if anchor.screen_row < metrics.viewport_offset as u32 {
            return None;
        }
        let viewport_row = anchor.screen_row - metrics.viewport_offset as u32;
        if viewport_row >= metrics.viewport_rows as u32 {
            return None;
        }
        Some((anchor.col, viewport_row as u16))
    }

    pub(crate) fn selected_text_from_anchors(
        &self,
        anchor: SelectionAnchor,
        cursor: SelectionAnchor,
    ) -> String {
        let ((start_col, start_row), (end_col, end_row)) = normalized_anchor_pair(anchor, cursor);
        self.terminal
            .read_text_screen((start_col, start_row), (end_col, end_row), false)
            .unwrap_or_default()
    }

    pub(crate) fn visible_viewport_selection(
        &self,
        anchor: SelectionAnchor,
        cursor: SelectionAnchor,
    ) -> Option<PaneSelection> {
        if anchor == cursor {
            return None;
        }
        let metrics = self.scroll_metrics()?;
        let offset = metrics.viewport_offset;
        let height = metrics.viewport_rows;
        if height == 0 {
            return None;
        }
        let (start, end) = if (anchor.screen_row, anchor.col) <= (cursor.screen_row, cursor.col) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let last_visible = offset + height.saturating_sub(1);
        if start.screen_row as usize > last_visible && end.screen_row as usize > last_visible {
            return None;
        }
        if (end.screen_row as usize) < offset && (start.screen_row as usize) < offset {
            return None;
        }

        let start_row = start
            .screen_row
            .saturating_sub(offset as u32)
            .min(height.saturating_sub(1) as u32) as u16;
        let end_row = end
            .screen_row
            .saturating_sub(offset as u32)
            .min(height.saturating_sub(1) as u32) as u16;
        let start_col = if (start.screen_row as usize) < offset {
            0
        } else {
            start.col
        };
        let end_col = if end.screen_row as usize > last_visible {
            self.cols.saturating_sub(1)
        } else {
            end.col
        };
        Some(PaneSelection {
            start: (start_col, start_row),
            end: (end_col, end_row),
        })
    }

    pub(crate) fn recent_plain_text(&self) -> String {
        self.viewport_plain_rows().join("\n")
    }

    pub(crate) fn cursor_cell(&self) -> Option<(u16, u16)> {
        if self.scrolled_up() {
            return None;
        }
        let cursor = self.render_state.cursor_viewport().ok()??;
        if self.render_state.cursor_visible().ok() != Some(true) {
            return None;
        }
        let col = cursor.x.min(self.cols.saturating_sub(1));
        let row = cursor.y.min(self.rows.saturating_sub(1));
        Some((col, row))
    }

    pub(crate) fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        let scrollbar = self.terminal.scrollbar().ok()?;
        Some(ScrollMetrics {
            viewport_offset: scrollbar.offset,
            offset_from_bottom: scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len),
            max_offset_from_bottom: scrollbar.total.saturating_sub(scrollbar.len),
            viewport_rows: scrollbar.len,
        })
    }

    fn set_scroll_offset_from_bottom(&mut self, lines: usize) {
        self.terminal.scroll_viewport_bottom();
        if lines > 0 {
            self.terminal.scroll_viewport_delta(-(lines as isize));
        }
        let _ = self.render_state.update(&self.terminal);
    }

    fn viewport_plain_rows(&self) -> Vec<String> {
        if self.rows == 0 {
            return Vec::new();
        }
        let end_row = u32::from(self.rows.saturating_sub(1));
        let end_col = self.cols.saturating_sub(1);
        let text = self
            .terminal
            .read_text_viewport((0, 0), (end_col, end_row), false)
            .unwrap_or_default();
        text.lines().map(str::to_string).collect()
    }

    fn input_state(&self) -> Option<InputState> {
        let bracketed_paste = self.terminal.mode_get(MODE_BRACKETED_PASTE).ok()?;
        let mouse_sgr = self.terminal.mode_get(MODE_MOUSE_SGR).ok()?;
        let mouse_utf8 = self.terminal.mode_get(MODE_MOUSE_UTF8).ok()?;
        let _mouse_alternate_scroll = self.terminal.mode_get(MODE_MOUSE_ALTERNATE_SCROLL).ok()?;
        let mouse_protocol_mode =
            if self.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
                MouseProtocolMode::AnyMotion
            } else if self.terminal.mode_get(MODE_MOUSE_BUTTON_MOTION).ok()? {
                MouseProtocolMode::ButtonMotion
            } else if self.terminal.mode_get(MODE_MOUSE_PRESS_RELEASE).ok()? {
                MouseProtocolMode::PressRelease
            } else if self.terminal.mode_get(MODE_MOUSE_X10).ok()? {
                MouseProtocolMode::Press
            } else {
                MouseProtocolMode::None
            };
        let mouse_protocol_encoding = if mouse_sgr {
            MouseProtocolEncoding::Sgr
        } else if mouse_utf8 {
            MouseProtocolEncoding::Utf8
        } else {
            MouseProtocolEncoding::Default
        };
        Some(InputState {
            bracketed_paste,
            mouse_protocol_mode,
            mouse_protocol_encoding,
        })
    }

    fn insert_tracked_text(&mut self, text: &str) {
        let idx = char_index_to_byte_index(&self.input_buffer, self.input_cursor);
        self.input_buffer.insert_str(idx, text);
        self.input_cursor = self.input_cursor.saturating_add(text.chars().count());
    }

    fn remove_tracked_char_before_cursor(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let start = char_index_to_byte_index(&self.input_buffer, self.input_cursor - 1);
        let end = char_index_to_byte_index(&self.input_buffer, self.input_cursor);
        self.input_buffer.replace_range(start..end, "");
        self.input_cursor = self.input_cursor.saturating_sub(1);
    }

    fn remove_tracked_char_at_cursor(&mut self) {
        let char_len = self.input_buffer.chars().count();
        if self.input_cursor >= char_len {
            return;
        }
        let start = char_index_to_byte_index(&self.input_buffer, self.input_cursor);
        let end = char_index_to_byte_index(&self.input_buffer, self.input_cursor + 1);
        self.input_buffer.replace_range(start..end, "");
    }

    fn finalize_tracked_command(&mut self) {
        let command = self.input_buffer.trim();
        if !command.is_empty() {
            self.last_command = Some(command.to_string());
            self.last_replayed_command = None;
            if let Some(agent_command) = agent_command_for_input(command) {
                self.set_command(agent_command.to_string());
            }
        }
        self.clear_pending_input();
    }
}

fn resize_pty(fd: RawFd, rows: u16, cols: u16) -> anyhow::Result<()> {
    let size = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn make_ghostty_state(
    rows: u16,
    cols: u16,
) -> anyhow::Result<(
    ghostty::Terminal,
    ghostty::RenderState,
    Option<ghostty::RgbColor>,
    Option<ghostty::RgbColor>,
)> {
    let mut terminal = ghostty::Terminal::new(cols, rows, SCROLLBACK_LINES)?;
    let _ = terminal.enable_grapheme_cluster_mode();
    let _ = terminal.enable_kitty_graphics();
    let mut render_state = ghostty::RenderState::new()?;
    let initial_colors = render_state
        .update(&terminal)
        .ok()
        .and_then(|_| render_state.colors().ok());
    Ok((
        terminal,
        render_state,
        initial_colors.map(|colors| colors.foreground),
        initial_colors.map(|colors| colors.background),
    ))
}

fn set_child_terminal_env() {
    unsafe {
        let term_name = CString::new("TERM").expect("TERM name");
        let term_value = CString::new(PANE_TERM).expect("TERM value");
        let colorterm_name = CString::new("COLORTERM").expect("COLORTERM name");
        let colorterm_value = CString::new(PANE_COLORTERM).expect("COLORTERM value");
        libc::setenv(term_name.as_ptr(), term_value.as_ptr(), 1);
        libc::setenv(colorterm_name.as_ptr(), colorterm_value.as_ptr(), 1);
    }
}

fn pane_cwd_from_pid(pid: i32) -> Option<PathBuf> {
    let path = format!("/proc/{pid}/cwd");
    std::fs::read_link(path).ok()
}

fn normalized_selection(selection: PaneSelection) -> ((u16, u16), (u16, u16)) {
    if (selection.start.1, selection.start.0) <= (selection.end.1, selection.end.0) {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    }
}

fn normalized_anchor_pair(
    anchor: SelectionAnchor,
    cursor: SelectionAnchor,
) -> ((u16, u32), (u16, u32)) {
    if (anchor.screen_row, anchor.col) <= (cursor.screen_row, cursor.col) {
        ((anchor.col, anchor.screen_row), (cursor.col, cursor.screen_row))
    } else {
        ((cursor.col, cursor.screen_row), (anchor.col, anchor.screen_row))
    }
}

fn mouse_modifier_bits(modifiers: KeyModifiers) -> u16 {
    let mut bits = 0u16;
    if modifiers.contains(KeyModifiers::SHIFT) {
        bits += 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        bits += 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        bits += 16;
    }
    bits
}

fn pump_pty_output(mut reader: File, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn exec_command(command: &str) -> ! {
    if command.trim().is_empty() {
        unsafe { libc::_exit(1) }
    }

    if !command.contains(char::is_whitespace) {
        let c = CString::new(command).unwrap();
        let args = [c.clone()];
        let _ = execvp(&c, &args);
        unsafe { libc::_exit(1) }
    }

    let shell = CString::new("/bin/sh").unwrap();
    let dash_c = CString::new("-c").unwrap();
    let cmd = CString::new(command).unwrap();
    let args = [shell.clone(), dash_c, cmd];
    let _ = execvp(&shell, &args);
    unsafe { libc::_exit(1) }
}

fn extract_resume_command(binary: &str, rows: &[String]) -> Option<String> {
    let resume_keywords = [
        "resume",
        "--session",
        "--continue",
        "--resume",
        " -r",
        " -c",
    ];

    for row in rows.iter().rev() {
        let trimmed = row.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let Some(start) = find_token(&lower, &binary.to_ascii_lowercase()) else {
            continue;
        };
        let tail_lower = &lower[start..];
        if !resume_keywords.iter().any(|kw| tail_lower.contains(kw)) {
            continue;
        }
        let candidate = trimmed[start..]
            .trim_end_matches(|c: char| {
                matches!(c, '`' | '\'' | '"' | ' ' | '.' | ',' | ')' | ']' | '\u{a0}')
            })
            .trim_start_matches(|c: char| matches!(c, '`' | '\'' | '"' | '('))
            .trim()
            .to_string();
        if !candidate.is_empty() {
            return Some(candidate);
        }
    }
    None
}

fn find_token(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let bytes = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = haystack[search_from..].find(needle) {
        let abs = search_from + rel;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let after = abs + needle.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return Some(abs);
        }
        search_from = abs + needle.len();
    }
    None
}

fn char_index_to_byte_index(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_codex_resume_line() {
        let rows = vec![
            "Bye!".to_string(),
            "To resume this session, run `codex resume abc-123-def`.".to_string(),
            "".to_string(),
        ];
        assert_eq!(
            extract_resume_command("codex", &rows).as_deref(),
            Some("codex resume abc-123-def")
        );
    }

    #[test]
    fn extracts_pi_session_line() {
        let rows = vec![
            "Session saved.".to_string(),
            "Resume with: pi --session 9f8e7d6c".to_string(),
        ];
        assert_eq!(
            extract_resume_command("pi", &rows).as_deref(),
            Some("pi --session 9f8e7d6c")
        );
    }

    #[test]
    fn ignores_substring_matches() {
        let rows = vec!["pipe --continue".to_string()];
        assert_eq!(extract_resume_command("pi", &rows), None);
    }

    #[test]
    fn requires_resume_keyword() {
        let rows = vec!["codex hello world".to_string()];
        assert_eq!(extract_resume_command("codex", &rows), None);
    }

    #[test]
    fn shell_output_reaches_ghostty_viewport() {
        let mut pane = Pane::new(
            10_001,
            "test",
            "sh -c 'echo code-ui-terminal-ok; exec cat'",
            None,
            None,
            24,
            80,
            None,
        )
        .expect("pane");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            pane.pump();
            if pane.recent_plain_text().contains("code-ui-terminal-ok") {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected shell output in viewport, got {:?}",
            pane.recent_plain_text()
        );
    }

    #[test]
    fn screen_selection_reads_scrolled_history() {
        let mut terminal = ghostty::Terminal::new(6, 3, 256).expect("terminal");
        for i in 0..8 {
            terminal.write(format!("row{i}\r\n").as_bytes());
        }
        terminal.scroll_viewport_delta(-2);
        let top = terminal.scrollbar().expect("scrollbar").offset as u32;
        let text = terminal
            .read_text_screen((0, top), (4, top + 2), false)
            .expect("selection text");
        assert!(
            text.contains("row3") || text.contains("row4") || text.contains("row5"),
            "unexpected selection text {text:?} at top={top}"
        );
    }

    #[test]
    fn repeated_scroll_up_moves_more_than_one_notch() {
        let mut terminal = ghostty::Terminal::new(80, 24, 256).expect("terminal");
        let mut render_state = ghostty::RenderState::new().expect("render state");
        for line in 0..40 {
            terminal.write(format!("line-{line:02}\r\n").as_bytes());
        }
        let _ = render_state.update(&terminal);
        let offset_from_bottom = |terminal: &ghostty::Terminal| {
            let scrollbar = terminal.scrollbar().expect("scrollbar");
            scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len)
        };
        assert_eq!(offset_from_bottom(&terminal), 0);
        for _ in 0..5 {
            terminal.scroll_viewport_delta(-3);
        }
        assert!(
            offset_from_bottom(&terminal) >= 15,
            "expected at least 15 lines of scrollback offset, got {}",
            offset_from_bottom(&terminal)
        );
    }

    #[test]
    fn resize_keeps_pty_and_terminal_in_sync() {
        let mut pane =
            Pane::new(10_002, "test", "cat", None, None, 12, 40, None).expect("pane");
        pane.pump();
        pane.resize(18, 100);
        assert_eq!(pane.rows, 18);
        assert_eq!(pane.cols, 100);
        pane.send(b"x").expect("write after resize");
    }

    #[test]
    fn prefers_most_recent_match() {
        let rows = vec![
            "old: codex resume aaaa".to_string(),
            "new: codex resume bbbb".to_string(),
            "".to_string(),
        ];
        assert_eq!(
            extract_resume_command("codex", &rows).as_deref(),
            Some("codex resume bbbb")
        );
    }
}
