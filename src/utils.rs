use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::layout::SplitSide;

/// Stored in layout/preset when the pane should spawn `$SHELL` (or `/bin/sh`).
pub(crate) const LOGIN_SHELL_SENTINEL: &str = "__SHELL__";

pub(crate) fn resolve_login_shell_command(command: &str) -> String {
    if command == LOGIN_SHELL_SENTINEL {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    } else {
        command.to_string()
    }
}

pub(crate) fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub(crate) fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => esc_seq(b"[Z"),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Left => esc_seq(b"[D"),
        KeyCode::Right => esc_seq(b"[C"),
        KeyCode::Up => esc_seq(b"[A"),
        KeyCode::Down => esc_seq(b"[B"),
        KeyCode::Home => esc_seq(b"[H"),
        KeyCode::End => esc_seq(b"[F"),
        KeyCode::PageUp => esc_seq(b"[5~"),
        KeyCode::PageDown => esc_seq(b"[6~"),
        KeyCode::Delete => esc_seq(b"[3~"),
        KeyCode::Insert => esc_seq(b"[2~"),
        KeyCode::F(n) if (1..=12).contains(&n) => esc_seq(match n {
            1 => b"[11~",
            2 => b"[12~",
            3 => b"[13~",
            4 => b"[14~",
            5 => b"[15~",
            6 => b"[17~",
            7 => b"[18~",
            8 => b"[19~",
            9 => b"[20~",
            10 => b"[21~",
            11 => b"[23~",
            12 => b"[24~",
            _ => unreachable!(),
        }),
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                ctrl_char(c)
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                let mut v = vec![0x1b];
                v.extend_from_slice(c.to_string().as_bytes());
                v
            } else {
                c.to_string().into_bytes()
            }
        }
        _ => Vec::new(),
    }
}

pub(crate) fn arrow_key_to_split_side(code: KeyCode) -> Option<SplitSide> {
    match code {
        KeyCode::Up => Some(SplitSide::Top),
        KeyCode::Down => Some(SplitSide::Bottom),
        KeyCode::Left => Some(SplitSide::Left),
        KeyCode::Right => Some(SplitSide::Right),
        _ => None,
    }
}

fn esc_seq(seq: &[u8]) -> Vec<u8> {
    let mut v = vec![0x1b];
    v.extend_from_slice(seq);
    v
}

fn ctrl_char(c: char) -> Vec<u8> {
    match c.to_ascii_lowercase() {
        'a'..='z' => vec![(c.to_ascii_lowercase() as u8) & 0x1f],
        ' ' => vec![0x00],
        '[' => vec![0x1b],
        '\\' => vec![0x1c],
        ']' => vec![0x1d],
        '^' => vec![0x1e],
        '_' => vec![0x1f],
        _ => Vec::new(),
    }
}
