use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use crate::{
    ui::agent_binary_for_command,
    utils::{resolve_login_shell_command, LOGIN_SHELL_SENTINEL},
};
use nix::{
    pty::{forkpty, ForkptyResult, Winsize},
    sys::wait::{waitpid, WaitPidFlag},
    unistd::{dup, execvp, Pid},
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::ScrollbarState,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

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
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) scrollback: usize,
    pub(crate) scrollback_max: usize,
    cached_view: Option<Text<'static>>,
    view_dirty: bool,
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
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<Self> {
        let command: String = command.into();
        // What we actually exec: the resume hint if we have one, otherwise the
        // canonical agent command (resolved through the shell sentinel).
        let exec_line = match resume_command.as_deref() {
            Some(line) => line.to_string(),
            None => resolve_login_shell_command(&command),
        };
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
                    agent_binary,
                    exited: false,
                    relaunch_failed: false,
                    parser: vt100::Parser::new(rows, cols, 2000),
                    writer,
                    rx,
                    child,
                    cols,
                    rows,
                    scrollback: 0,
                    scrollback_max: 0,
                    cached_view: None,
                    view_dirty: true,
                };
                pane.sync_scrollback();
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

    /// Send SIGTERM to the child process group. Used by the graceful-shutdown
    /// path to give the agent a chance to print its resume hint before we
    /// tear everything down.
    pub(crate) fn request_exit(&self) {
        unsafe {
            libc::kill(self.child.as_raw(), libc::SIGTERM);
        }
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
        let agent_binary = self.agent_binary;

        let mut new = Pane::new(
            self.id,
            self.title.clone(),
            LOGIN_SHELL_SENTINEL,
            None,
            self.rows.max(1),
            self.cols.max(1),
        )?;
        new.command = command;
        new.resume_command = resume_command;
        new.agent_binary = agent_binary;

        // Drop sends SIGTERM/waitpid for the old (already-exited) child.
        *self = new;
        Ok(())
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
        let x = x.saturating_add(1);
        let y = y.saturating_add(1);
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

    fn sync_scrollback(&mut self) {
        let desired = self.scrollback;
        let max_safe_offset = self.rows as usize;

        self.parser.set_scrollback(usize::MAX);
        self.scrollback_max = self.parser.screen().scrollback().min(max_safe_offset);
        self.scrollback = desired.min(self.scrollback_max);
        self.parser.set_scrollback(self.scrollback);
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) {
        let max_safe_offset = self.rows as usize;
        let next = if delta.is_negative() {
            self.scrollback.saturating_sub((-delta) as usize)
        } else {
            self.scrollback.saturating_add(delta as usize)
        };
        let new_scrollback = next.min(self.scrollback_max).min(max_safe_offset);
        if new_scrollback != self.scrollback {
            self.scrollback = new_scrollback;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
        }
    }

    pub(crate) fn scroll_up(&mut self) {
        self.scroll_by(1);
    }

    pub(crate) fn scroll_down(&mut self) {
        self.scroll_by(-1);
    }

    pub(crate) fn page_up(&mut self) {
        self.scroll_by(self.rows.max(1) as isize);
    }

    pub(crate) fn page_down(&mut self) {
        self.scroll_by(-(self.rows.max(1) as isize));
    }

    pub(crate) fn scroll_top(&mut self) {
        let target = self.scrollback_max.min(self.rows as usize);
        if target != self.scrollback {
            self.scrollback = target;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
        }
    }

    pub(crate) fn scroll_bottom(&mut self) {
        if self.scrollback != 0 {
            self.scrollback = 0;
            self.parser.set_scrollback(self.scrollback);
            self.view_dirty = true;
        }
    }

    pub(crate) fn needs_scrollbar(&self, viewport_width: u16, viewport_height: u16) -> bool {
        if self.scrollback_max > 0 {
            return true;
        }

        self.rendered_height(viewport_width) > usize::from(viewport_height)
    }

    pub(crate) fn scrollbar_state(
        &self,
        viewport_width: u16,
        viewport_height: u16,
    ) -> ScrollbarState {
        if self.scrollback_max > 0 {
            return ScrollbarState::new(self.scrollback_max.saturating_add(1))
                .position(self.scrollback_max.saturating_sub(self.scrollback))
                .viewport_content_length(usize::from(viewport_height));
        }

        ScrollbarState::new(self.rendered_height(viewport_width))
            .position(0)
            .viewport_content_length(usize::from(viewport_height))
    }

    /// Returns the rendered terminal contents as a styled `Text`. The result is
    /// cached and only rebuilt when the underlying screen has actually changed
    /// (new PTY bytes, scroll, or resize). When clean, this just clones the
    /// cached value.
    pub(crate) fn styled_view(&mut self) -> Text<'static> {
        if !self.view_dirty {
            if let Some(cached) = &self.cached_view {
                return cached.clone();
            }
        }
        let text = self.build_styled_view();
        self.cached_view = Some(text.clone());
        self.view_dirty = false;
        text
    }

    fn build_styled_view(&self) -> Text<'static> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
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
                let style = cell_style(cell);

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
        if self.scrollback > 0 {
            return None;
        }

        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }

        let (row, col) = screen.cursor_position();
        let row = row.min(area.height.saturating_sub(1));
        let col = col.min(area.width.saturating_sub(1));

        Some((area.x + col, area.y + row))
    }

    fn rendered_height(&self, viewport_width: u16) -> usize {
        let viewport_width = usize::from(viewport_width.max(1));
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut height = 0usize;

        for row in 0..rows {
            let mut row_width = 0usize;
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.has_contents() {
                    let cell_width = if cell.is_wide() { 2 } else { 1 };
                    row_width = row_width.max(usize::from(col) + cell_width);
                }
            }

            height = height.saturating_add(row_width.max(1).div_ceil(viewport_width));
        }

        height
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut fg = vt100_color_to_tui(cell.fgcolor());
    let mut bg = vt100_color_to_tui(cell.bgcolor());

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
}
