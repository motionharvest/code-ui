use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::mpsc::{self, Receiver},
    thread,
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

pub(crate) struct Pane {
    pub(crate) id: usize,
    pub(crate) title: String,
    pub(crate) parser: vt100::Parser,
    writer: File,
    rx: Receiver<Vec<u8>>,
    child: Pid,
    cols: u16,
    pub(crate) rows: u16,
    pub(crate) scrollback: usize,
    pub(crate) scrollback_max: usize,
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
    pub(crate) fn new(id: usize, title: impl Into<String>, rows: u16, cols: u16) -> anyhow::Result<Self> {
        let ws = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let fork_result = unsafe { forkpty(Some(&ws), None)? };

        match fork_result {
            ForkptyResult::Child => {
                exec_pi();
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
                    parser: vt100::Parser::new(rows, cols, 2000),
                    writer,
                    rx,
                    child,
                    cols,
                    rows,
                    scrollback: 0,
                    scrollback_max: 0,
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
    }

    pub(crate) fn pump(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            self.parser.process(&bytes);
        }
        self.sync_scrollback();
    }

    pub(crate) fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
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
        self.scrollback = next.min(self.scrollback_max).min(max_safe_offset);
        self.parser.set_scrollback(self.scrollback);
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
        self.scrollback = self.scrollback_max.min(self.rows as usize);
        self.parser.set_scrollback(self.scrollback);
    }

    pub(crate) fn scroll_bottom(&mut self) {
        self.scrollback = 0;
        self.parser.set_scrollback(self.scrollback);
    }

    pub(crate) fn scrollbar_state(&self) -> ScrollbarState {
        ScrollbarState::new(self.scrollback_max.saturating_add(1))
            .position(self.scrollback_max.saturating_sub(self.scrollback))
            .viewport_content_length(self.rows as usize)
    }

    pub(crate) fn styled_view(&self) -> Text<'static> {
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

fn exec_pi() -> ! {
    let pi = std::ffi::CString::new("pi").unwrap();
    let args = [pi.clone()];

    let _ = execvp(&pi, &args);
    unsafe { libc::_exit(1) }
}
