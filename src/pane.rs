use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::{
    ui::{agent_binary_for_command, agent_command_for_input},
    utils::{resolve_login_shell_command, LOGIN_SHELL_SENTINEL},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use nix::{
    pty::{forkpty, ForkptyResult, Winsize},
    sys::wait::{waitpid, WaitPidFlag},
    unistd::{dup, execvp, Pid},
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Match tmux `history-limit` and `capture-pane` replay depth.
const TMUX_HISTORY_LIMIT: &str = "50000";
const TMUX_HISTORY_LIMIT_USIZE: usize = 50_000;

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
    /// Canonical agent command (e.g. "codex", "pi", or the shell sentinel).
    /// Used for agent-index lookup and the settings UI. The line actually
    /// exec'd may differ if a `resume_command` was supplied.
    pub(crate) command: String,
    /// Captured resume hint (e.g. "codex resume abc-123"). Populated when the
    /// child process exits and the pane's output contained a recognizable
    /// resume line. Persisted so the pane can be brought back on next launch.
    pub(crate) resume_command: Option<String>,
    /// Most recent command submitted by the user in this pane.
    pub(crate) last_command: Option<String>,
    /// Binary token used for scraping resume hints; `None` disables capture.
    agent_binary: Option<&'static str>,
    /// Set once we've observed the PTY reader thread disconnect.
    pub(crate) exited: bool,
    /// Sticky flag set when an attempt to respawn the pane (e.g. after the
    /// agent exits) fails. Prevents tight retry loops on persistent forkpty
    /// errors.
    pub(crate) relaunch_failed: bool,
    pub(crate) parser: vt100::Parser,
    writer: File,
    rx: Receiver<Vec<u8>>,
    child: Pid,
    tmux_session: String,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) scrollback: usize,
    pub(crate) scrollback_max: usize,
    last_replayed_command: Option<String>,
    input_buffer: String,
    input_cursor: usize,
    cached_view: Option<Text<'static>>,
    view_dirty: bool,
    first_paint_pending: bool,
}

impl Drop for Pane {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.as_raw(), libc::SIGTERM);
        }
        let _ = waitpid(self.child, Some(WaitPidFlag::WNOHANG));
    }
}

impl Pane {
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
        // What we actually run inside the persistent tmux session: the resume
        // hint if we have one, otherwise the canonical agent command.
        let pane_command = match resume_command.as_deref() {
            Some(line) => line.to_string(),
            None => resolve_login_shell_command(&command),
        };
        let tmux_session = tmux_session_name(id);
        ensure_tmux_session(&tmux_session, &pane_command)?;
        let session_q = shell_quote(&tmux_session);
        let command_q = shell_quote(&pane_command);
        let exec_line = format!(
            "tmux attach-session -t {session} 2>/dev/null || \
             (tmux new-session -d -s {session} {command} >/dev/null 2>&1 && \
              tmux set-option -t {session} status off >/dev/null 2>&1 && \
              tmux attach-session -t {session} 2>/dev/null)",
            session = session_q,
            command = command_q
        );
        let agent_binary = agent_binary_for_command(&command);
        let ws = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let fork_result = unsafe { forkpty(Some(&ws), None)? };

        match fork_result {
            ForkptyResult::Child => {
                exec_command(&exec_line);
            }
            ForkptyResult::Parent { child, master } => {
                let reader_fd = unsafe { OwnedFd::from_raw_fd(dup(master.as_raw_fd())?) };
                let writer = File::from(master);
                let reader = File::from(reader_fd);
                let (tx, rx) = mpsc::channel();

                thread::spawn(move || pump_pty_output(reader, tx));

                let mut pane = Self {
                    id,
                    title: title.into(),
                    command,
                    resume_command,
                    last_command,
                    agent_binary,
                    exited: false,
                    relaunch_failed: false,
                    parser: vt100::Parser::new(rows, cols, TMUX_HISTORY_LIMIT_USIZE),
                    writer,
                    rx,
                    child,
                    tmux_session,
                    cols,
                    rows,
                    scrollback: 0,
                    scrollback_max: 0,
                    last_replayed_command: None,
                    input_buffer: String::new(),
                    input_cursor: 0,
                    cached_view: None,
                    view_dirty: true,
                    first_paint_pending: true,
                };
                pane.replay_tmux_history();
                pane.sync_scrollback();
                if let Some(offset) = initial_scroll_offset.filter(|offset| *offset > 0) {
                    let clamped = offset.min(pane.scrollback_max);
                    if clamped > 0 {
                        pane.scrollback = clamped;
                        pane.parser.set_scrollback(clamped);
                    }
                    restore_tmux_scroll_position(&pane.tmux_session, offset);
                }
                Ok(pane)
            }
        }
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols {
            return;
        }

        self.rows = rows;
        self.cols = cols;

        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        unsafe {
            libc::ioctl(self.writer.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
        self.parser.set_size(rows, cols);
        self.sync_scrollback();
        self.view_dirty = true;
    }

    /// Drain any pending PTY output into the parser. Returns true if any bytes
    /// were processed (i.e. the rendered view may have changed).
    pub(crate) fn pump(&mut self) -> bool {
        if self.first_paint_pending {
            return false;
        }
        let mut processed = false;
        let mut disconnected = false;
        loop {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    self.parser.process(&bytes);
                    processed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if processed {
            self.sync_scrollback();
            self.view_dirty = true;
        }
        if disconnected && !self.exited {
            self.exited = true;
            self.capture_resume_command();
            // The exit transition is itself a visual change worth redrawing.
            self.view_dirty = true;
            processed = true;
        }
        processed
    }

    /// Called by the UI renderer once the pane has been painted at least once.
    /// New panes intentionally delay PTY output processing until this point so
    /// tmux attach redraw artifacts don't flash before the pane frame appears.
    pub(crate) fn mark_painted(&mut self) {
        self.first_paint_pending = false;
    }

    /// Permanently close the persistent session backing this pane. This is
    /// used when the user closes/replaces a pane, not when the whole app exits.
    pub(crate) fn terminate_session(&self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.tmux_session])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        wait_for_tmux_session_gone(&self.tmux_session, Duration::from_millis(300));
    }

    /// Spawn a fresh login shell into this pane, replacing the previous PTY
    /// child. Called after the prior program (agent or shell) exits so the
    /// pane stays usable instead of becoming a frozen dead view.
    ///
    /// The shell is only the replacement process. Keep the pane metadata
    /// (`command`, `resume_command`, and `agent_binary`) pointed at the
    /// original agent so persistence can bring the pane back where the agent
    /// left off on the next launch.
    pub(crate) fn relaunch_as_shell(&mut self) -> anyhow::Result<()> {
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
            self.rows.max(1),
            self.cols.max(1),
            None,
        )?;
        new.command = command;
        new.resume_command = resume_command;
        new.last_command = last_command;
        new.last_replayed_command = last_replayed_command;
        new.agent_binary = agent_binary;
        let _ = new.replay_last_command();

        // Drop sends SIGTERM/waitpid for the old (already-exited) child.
        *self = new;
        Ok(())
    }

    pub(crate) fn tmux_pane_path(&self) -> Option<std::path::PathBuf> {
        let output = Command::new("tmux")
            .args([
                "display-message",
                "-p",
                "-t",
                &self.tmux_session,
                "#{pane_current_path}",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            return None;
        }
        Some(std::path::PathBuf::from(path))
    }

    /// Scroll offset to persist: local vt100 viewport plus tmux copy-mode position.
    pub(crate) fn persisted_scroll_offset(&self) -> usize {
        self.scrollback.max(read_tmux_scroll_position(&self.tmux_session))
    }

    fn replay_tmux_history(&mut self) {
        let start = format!("-{TMUX_HISTORY_LIMIT}");
        let Ok(output) = Command::new("tmux")
            .args([
                "capture-pane",
                "-p",
                "-e",
                "-J",
                "-S",
                &start,
                "-t",
                &self.tmux_session,
            ])
            .output()
        else {
            return;
        };
        if output.status.success() && !output.stdout.is_empty() {
            self.parser.process(&output.stdout);
            if !output.stdout.ends_with(b"\n") {
                self.parser.process(b"\n");
            }
            self.view_dirty = true;
        }
    }

    /// Best-effort: scan whatever is currently on screen and store a resume
    /// hint if one is visible. Idempotent and safe to call repeatedly; only
    /// overwrites `resume_command` when a match is found.
    pub(crate) fn try_capture_resume_command(&mut self) {
        self.capture_resume_command();
    }

    /// Scan the rendered terminal contents for the most recent line that
    /// looks like a resume hint emitted by the agent and store it in
    /// `self.resume_command`. No-op for panes without an agent binary.
    fn capture_resume_command(&mut self) {
        let Some(binary) = self.agent_binary else {
            return;
        };

        // Temporarily clear the scrollback offset so we see the most recent
        // output regardless of where the user scrolled.
        let saved = self.scrollback;
        self.parser.set_scrollback(0);
        let rows: Vec<String> = {
            let screen = self.parser.screen();
            let (_, cols) = screen.size();
            screen.rows(0, cols).collect()
        };
        self.parser.set_scrollback(saved);

        if let Some(line) = extract_resume_command(binary, &rows) {
            self.resume_command = Some(line);
        }
    }

    pub(crate) fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub(crate) fn send_paste(&mut self, text: &str) -> anyhow::Result<()> {
        if self.parser.screen().bracketed_paste() {
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
        let screen = self.parser.screen();
        if screen.mouse_protocol_mode() == MouseProtocolMode::None {
            return Ok(false);
        }

        let button = if up { 64 } else { 65 };
        let x = x.min(self.cols.saturating_sub(1)).saturating_add(1);
        let y = y.min(self.rows.saturating_sub(1)).saturating_add(1);
        let bytes = match screen.mouse_protocol_encoding() {
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
        let screen = self.parser.screen();
        let mode = screen.mouse_protocol_mode();
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
            && !matches!(
                mode,
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
            )
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
            PaneMouseEventKind::Up => match screen.mouse_protocol_encoding() {
                MouseProtocolEncoding::Sgr => button_code + modifier_bits,
                MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => 3 + modifier_bits,
            },
        };

        let x = x.min(self.cols.saturating_sub(1)).saturating_add(1);
        let y = y.min(self.rows.saturating_sub(1)).saturating_add(1);

        let bytes = match screen.mouse_protocol_encoding() {
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
                // Shell history state isn't observable to us, so avoid
                // carrying stale partially-typed input across history jumps.
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

    pub(crate) fn set_command(&mut self, command: String) {
        self.command = command;
        self.agent_binary = agent_binary_for_command(&self.command);
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

    fn sync_scrollback(&mut self) {
        let desired = self.scrollback;

        self.parser.set_scrollback(usize::MAX);
        self.scrollback_max = self.parser.screen().scrollback();
        self.scrollback = desired.min(self.scrollback_max);
        self.parser.set_scrollback(self.scrollback);
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) -> bool {
        let next = if delta.is_negative() {
            self.scrollback.saturating_sub((-delta) as usize)
        } else {
            self.scrollback.saturating_add(delta as usize)
        };
        let new_scrollback = next.min(self.scrollback_max);
        if new_scrollback != self.scrollback {
            self.scrollback = new_scrollback;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
            return true;
        }
        false
    }

    pub(crate) fn scroll_up(&mut self) -> bool {
        if self.scroll_by(1) {
            return true;
        }
        self.tmux_scroll_lines(1, TmuxScrollDirection::Up)
    }

    pub(crate) fn scroll_down(&mut self) -> bool {
        if self.scroll_by(-1) {
            return true;
        }
        self.tmux_scroll_lines(1, TmuxScrollDirection::Down)
    }

    pub(crate) fn page_up(&mut self) -> bool {
        let lines = self.rows.max(1) as usize;
        if self.scroll_by(lines as isize) {
            return true;
        }
        self.tmux_scroll_lines(lines, TmuxScrollDirection::Up)
    }

    pub(crate) fn page_down(&mut self) -> bool {
        let lines = self.rows.max(1) as usize;
        if self.scroll_by(-(lines as isize)) {
            return true;
        }
        self.tmux_scroll_lines(lines, TmuxScrollDirection::Down)
    }

    pub(crate) fn scroll_top(&mut self) -> bool {
        if self.scrollback_max > 0 && self.scrollback < self.scrollback_max {
            self.scrollback = self.scrollback_max;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
            return true;
        }
        self.tmux_scroll_to_history_edge(true)
    }

    pub(crate) fn scroll_bottom(&mut self) -> bool {
        let mut changed = false;
        if self.scrollback > 0 {
            self.scrollback = 0;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
            changed = true;
        }
        changed | self.tmux_scroll_to_history_edge(false)
    }

    fn tmux_scroll_lines(&mut self, lines: usize, direction: TmuxScrollDirection) -> bool {
        if lines == 0 {
            return false;
        }
        if !tmux_enter_copy_mode(&self.tmux_session) {
            return false;
        }
        let command = match direction {
            TmuxScrollDirection::Up => "scroll-up",
            TmuxScrollDirection::Down => "scroll-down",
        };
        if !tmux_send_keys_x(
            &self.tmux_session,
            &["-N", &lines.to_string(), command],
        ) {
            return false;
        }
        self.view_dirty = true;
        true
    }

    fn tmux_scroll_to_history_edge(&mut self, top: bool) -> bool {
        if !tmux_enter_copy_mode(&self.tmux_session) {
            return false;
        }
        let command = if top { "history-top" } else { "history-bottom" };
        if !tmux_send_keys_x(&self.tmux_session, &[command]) {
            return false;
        }
        self.view_dirty = true;
        true
    }

    /// Returns the rendered terminal contents as a styled `Text`. The result is
    /// cached and only rebuilt when the underlying screen has actually changed
    /// (new PTY bytes, scroll, or resize). When clean, this just clones the
    /// cached value.
    pub(crate) fn styled_view(&mut self, selection: Option<PaneSelection>) -> Text<'static> {
        if let Some(selection) = selection {
            return self.build_styled_view(Some(selection));
        }

        if !self.view_dirty {
            if let Some(cached) = &self.cached_view {
                return cached.clone();
            }
        }
        let text = self.build_styled_view(None);
        self.cached_view = Some(text.clone());
        self.view_dirty = false;
        text
    }

    pub(crate) fn selected_text(&self, selection: PaneSelection) -> String {
        let ((start_col, start_row), (end_col, end_row)) = normalized_selection(selection);
        self.parser.screen().contents_between(
            start_row,
            start_col,
            end_row,
            end_col.saturating_add(1),
        )
    }

    pub(crate) fn recent_plain_text(&mut self) -> String {
        let saved = self.scrollback;
        self.parser.set_scrollback(0);
        let rows: Vec<String> = {
            let screen = self.parser.screen();
            let (_, cols) = screen.size();
            screen.rows(0, cols).collect()
        };
        self.parser.set_scrollback(saved);
        rows.join("\n")
    }

    fn build_styled_view(&self, selection: Option<PaneSelection>) -> Text<'static> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let normalized = selection.map(normalized_selection);
        let mut lines = Vec::with_capacity(usize::from(rows));

        for row in 0..rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut current_style: Option<Style> = None;
            let mut current_text = String::new();

            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }

                let text = if cell.has_contents() {
                    cell.contents()
                } else {
                    " ".to_string()
                };
                let mut style = cell_style(cell);
                if selection_contains(normalized, col, row) {
                    style = style.fg(Color::Black).bg(Color::White);
                }

                if current_style == Some(style) {
                    current_text.push_str(&text);
                } else {
                    if !current_text.is_empty() {
                        let span = if let Some(style) = current_style.take() {
                            Span::styled(std::mem::take(&mut current_text), style)
                        } else {
                            Span::raw(std::mem::take(&mut current_text))
                        };
                        spans.push(span);
                    }
                    current_style = Some(style);
                    current_text.push_str(&text);
                }
            }

            if !current_text.is_empty() {
                let span = if let Some(style) = current_style.take() {
                    Span::styled(current_text, style)
                } else {
                    Span::raw(current_text)
                };
                spans.push(span);
            }

            lines.push(Line::from(spans));
        }

        Text::from(lines)
    }

    pub(crate) fn cursor_position_in(&self, area: Rect) -> Option<(u16, u16)> {
        let (col, row) = self.cursor_cell()?;
        Some((area.x + col, area.y + row))
    }

    pub(crate) fn cursor_cell(&self) -> Option<(u16, u16)> {
        if self.scrollback > 0 {
            return None;
        }

        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }

        let (rows, cols) = screen.size();
        let (row, col) = screen.cursor_position();
        let row = row.min(rows.saturating_sub(1));
        let col = col.min(cols.saturating_sub(1));

        Some((col, row))
    }
}

fn normalized_selection(selection: PaneSelection) -> ((u16, u16), (u16, u16)) {
    if (selection.start.1, selection.start.0) <= (selection.end.1, selection.end.0) {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    }
}

fn selection_contains(selection: Option<((u16, u16), (u16, u16))>, col: u16, row: u16) -> bool {
    let Some(((start_col, start_row), (end_col, end_row))) = selection else {
        return false;
    };

    if row < start_row || row > end_row {
        return false;
    }
    if start_row == end_row {
        return col >= start_col && col <= end_col;
    }
    if row == start_row {
        return col >= start_col;
    }
    if row == end_row {
        return col <= end_col;
    }
    true
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

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut fg = vt100_color_to_tui(cell.fgcolor());
    // Leave default terminal cells unset so the pane widget's base background
    // can show through.
    let mut bg = match cell.bgcolor() {
        vt100::Color::Default => None,
        color => vt100_color_to_tui(color),
    };

    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut style = Style::default();
    if let Some(color) = fg {
        style = style.fg(color);
    }
    if let Some(color) = bg {
        style = style.bg(color);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }

    style
}

fn vt100_color_to_tui(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
    }
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

#[derive(Clone, Copy)]
enum TmuxScrollDirection {
    Up,
    Down,
}

fn tmux_target(session: &str) -> String {
    session.to_string()
}

fn tmux_run(args: &[&str]) -> bool {
    Command::new("tmux")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn tmux_enter_copy_mode(session: &str) -> bool {
    tmux_run(&["copy-mode", "-t", &tmux_target(session)])
}

fn tmux_send_keys_x(session: &str, command_args: &[&str]) -> bool {
    let target = tmux_target(session);
    let mut args = vec!["send-keys", "-t", target.as_str(), "-X"];
    args.extend_from_slice(command_args);
    tmux_run(&args)
}

fn read_tmux_scroll_position(session: &str) -> usize {
    let target = tmux_target(session);
    let Ok(output) = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            target.as_str(),
            "-F",
            "#{scroll_position}",
        ])
        .output()
    else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn restore_tmux_scroll_position(session: &str, offset: usize) {
    if offset == 0 {
        return;
    }
    if !tmux_enter_copy_mode(session) {
        return;
    }
    let _ = tmux_send_keys_x(session, &["-N", &offset.to_string(), "scroll-up"]);
}

fn configure_tmux_session(session: &str) {
    for (option, value) in [
        ("status", "off"),
        ("history-limit", TMUX_HISTORY_LIMIT),
        ("mouse", "on"),
    ] {
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, option, value])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn ensure_tmux_session(session: &str, command: &str) -> anyhow::Result<()> {
    let has_session = Command::new("tmux")
        .args(["has-session", "-t", session])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !has_session {
        let status = Command::new("tmux")
            .args(["new-session", "-d", "-s", session, command])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            anyhow::bail!("failed to create tmux session {session}");
        }
    }

    configure_tmux_session(session);
    Ok(())
}

fn wait_for_tmux_session_gone(session: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let exists = Command::new("tmux")
            .args(["has-session", "-t", session])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !exists {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn tmux_session_name(pane_id: usize) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut hash = 0xcbf29ce484222325u64;
    for byte in cwd.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("codeui-{hash:016x}-pane-{pane_id}")
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
    // If the line contains no whitespace, treat it as a single binary name and
    // exec directly (preserves prior behavior for the common case). Otherwise
    // delegate to /bin/sh -c so multi-arg resume commands like
    // `codex resume <id>` work correctly.
    if command.trim().is_empty() {
        unsafe { libc::_exit(1) }
    }

    if !command.contains(char::is_whitespace) {
        let c = std::ffi::CString::new(command).unwrap();
        let args = [c.clone()];
        let _ = execvp(&c, &args);
        unsafe { libc::_exit(1) }
    }

    let shell = std::ffi::CString::new("/bin/sh").unwrap();
    let dash_c = std::ffi::CString::new("-c").unwrap();
    let cmd = std::ffi::CString::new(command).unwrap();
    let args = [shell.clone(), dash_c, cmd];
    let _ = execvp(&shell, &args);
    unsafe { libc::_exit(1) }
}

/// Scan rendered rows from bottom to top for the most recent line containing
/// `binary` as a token and at least one resume-style keyword. The captured
/// substring is the binary occurrence through the end of the trimmed line,
/// stripped of common surrounding punctuation (backticks, quotes, parens).
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

/// Return the byte offset of `needle` inside `haystack` where the surrounding
/// characters are non-alphanumeric (so we don't match "pipe" when looking for
/// "pi").
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
        // "pipe" must not be mistaken for the pi binary.
        let rows = vec!["pipe --continue".to_string()];
        assert_eq!(extract_resume_command("pi", &rows), None);
    }

    #[test]
    fn requires_resume_keyword() {
        let rows = vec!["codex hello world".to_string()];
        assert_eq!(extract_resume_command("codex", &rows), None);
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

    #[test]
    fn default_background_leaves_base_style_visible() {
        let mut parser = vt100::Parser::new(1, 1, 0);
        parser.process(b"A");
        let cell = parser.screen().cell(0, 0).expect("cell");

        assert_eq!(cell_style(cell).bg, None);
    }
}
