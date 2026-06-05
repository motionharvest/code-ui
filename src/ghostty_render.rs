//! Render a Ghostty terminal into a ratatui frame buffer (adapted from Herdr).

use ratatui::style::{Color, Modifier, Style};
use ratatui::{layout::Rect, Frame};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn render_pane(
    terminal: &crate::ghostty::Terminal,
    render_state: &mut crate::ghostty::RenderState,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial_default_foreground: Option<crate::ghostty::RgbColor>,
    initial_default_background: Option<crate::ghostty::RgbColor>,
    frame: &mut Frame,
    area: Rect,
    show_cursor: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if render_state.update(terminal).is_err() {
        return;
    }
    let colors = render_state.colors().ok();
    let default_bg = colors
        .and_then(|c| ghostty_default_bg(c.background, host_theme, initial_default_background));
    let default_fg = colors
        .and_then(|c| ghostty_default_fg(c.foreground, host_theme, initial_default_foreground));
    let resolved_fg = colors.map(|c| ghostty_color(c.foreground));
    let resolved_bg = colors.map(|c| ghostty_color(c.background));

    let mut row_iterator = match crate::ghostty::RowIterator::new() {
        Ok(iterator) => iterator,
        Err(_) => return,
    };
    let mut row_cells = match crate::ghostty::RowCells::new() {
        Ok(cells) => cells,
        Err(_) => return,
    };

    let buf = frame.buffer_mut();
    let mut rows = match render_state.populate_row_iterator(&mut row_iterator) {
        Ok(rows) => rows,
        Err(_) => return,
    };
    let mut grapheme_codepoints = Vec::new();
    let mut symbol_scratch = String::new();
    let mut y = 0u16;
    while y < area.height && rows.next() {
        let mut cells = match rows.populate_cells(&mut row_cells) {
            Ok(cells) => cells,
            Err(_) => break,
        };
        let mut x = 0u16;
        while x < area.width && cells.next() {
            let basic = cells.basic_data().unwrap_or_default();
            let style = ghostty_cell_style(
                &cells,
                &basic,
                default_fg,
                default_bg,
                resolved_fg,
                resolved_bg,
            );
            let symbol = match ghostty_buffer_symbol_into(
                &cells,
                basic.wide,
                &mut grapheme_codepoints,
                &mut symbol_scratch,
            ) {
                Ok(symbol) => symbol,
                Err(_) => {
                    symbol_scratch.clear();
                    symbol_scratch.push_str(ghostty_blank_symbol_for_width(basic.wide));
                    symbol_scratch.as_str()
                }
            };
            let cell = buf.get_mut(area.x + x, area.y + y);
            cell.reset();
            cell.set_symbol(symbol);
            cell.set_style(style);
            x += 1;
        }
        while x < area.width {
            ghostty_reset_cell(buf.get_mut(area.x + x, area.y + y), default_fg, default_bg);
            x += 1;
        }
        y += 1;
    }
    while y < area.height {
        for x in 0..area.width {
            ghostty_reset_cell(buf.get_mut(area.x + x, area.y + y), default_fg, default_bg);
        }
        y += 1;
    }

    ghostty_clear_render_dirty(render_state, area.height);

    if show_cursor && render_state.cursor_visible().ok() == Some(true) {
        if let Ok(Some(cursor)) = render_state.cursor_viewport() {
            if cursor.x < area.width && cursor.y < area.height {
                frame.set_cursor(area.x + cursor.x, area.y + cursor.y);
            }
        }
    }
}

fn ghostty_clear_render_dirty(render_state: &mut crate::ghostty::RenderState, area_height: u16) {
    let Ok(mut row_iterator) = crate::ghostty::RowIterator::new() else {
        return;
    };
    let Ok(mut rows) = render_state.populate_row_iterator(&mut row_iterator) else {
        return;
    };
    let mut y = 0u16;
    while y < area_height && rows.next() {
        let _ = rows.clear_dirty();
        y += 1;
    }
    let _ = render_state.set_dirty(crate::ghostty::Dirty::Clean);
}

fn ghostty_blank_symbol_for_width(wide: crate::ghostty::CellWide) -> &'static str {
    match wide {
        crate::ghostty::CellWide::Wide => "  ",
        crate::ghostty::CellWide::SpacerTail => "",
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => " ",
    }
}

fn ghostty_buffer_symbol_into<'a>(
    cells: &crate::ghostty::RowCellIter<'_>,
    wide: crate::ghostty::CellWide,
    grapheme_codepoints: &mut Vec<u32>,
    symbol_scratch: &'a mut String,
) -> Result<&'a str, crate::ghostty::Error> {
    symbol_scratch.clear();
    match wide {
        crate::ghostty::CellWide::SpacerTail => {}
        crate::ghostty::CellWide::SpacerHead => symbol_scratch.push(' '),
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::Wide => {
            cells.grapheme_text_into(grapheme_codepoints, symbol_scratch)?;
            let hidden = symbol_scratch.chars().next().map(u32::from)
                == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER);
            if hidden || symbol_scratch.is_empty() {
                symbol_scratch.clear();
                symbol_scratch.push(' ');
            }
        }
    }
    let expected_width = match wide {
        crate::ghostty::CellWide::Wide => 2,
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => 1,
        crate::ghostty::CellWide::SpacerTail => 0,
    };
    if symbol_scratch.width() != expected_width
        && !(wide == crate::ghostty::CellWide::Narrow && symbol_scratch.width() == 2)
    {
        symbol_scratch.clear();
        symbol_scratch.push_str(ghostty_blank_symbol_for_width(wide));
    }
    Ok(symbol_scratch.as_str())
}

fn ghostty_reset_cell(cell: &mut ratatui::buffer::Cell, default_fg: Option<Color>, default_bg: Option<Color>) {
    cell.reset();
    cell.set_symbol(" ");
    if let Some(fg) = default_fg {
        cell.set_fg(fg);
    }
    if let Some(bg) = default_bg {
        cell.set_bg(bg);
    }
}

fn ghostty_cell_style(
    cells: &crate::ghostty::RowCellIter<'_>,
    basic: &crate::ghostty::CellBasicData,
    default_fg: Option<Color>,
    default_bg: Option<Color>,
    resolved_fg: Option<Color>,
    resolved_bg: Option<Color>,
) -> Style {
    let mut fg = basic
        .style
        .fg_color
        .map(ghostty_cell_color)
        .or_else(|| cells.fg_color().ok().flatten().map(ghostty_color))
        .or(default_fg);
    let mut bg = cells
        .content_bg_color()
        .ok()
        .flatten()
        .or(basic.style.bg_color)
        .map(ghostty_cell_color)
        .or_else(|| cells.bg_color().ok().flatten().map(ghostty_color))
        .or(default_bg);
    if basic.style.invisible {
        fg = bg.or(default_bg);
    }
    if basic.style.inverse {
        if bg.is_none() {
            bg = resolved_bg;
        }
        if fg.is_none() {
            fg = resolved_fg;
        }
        std::mem::swap(&mut fg, &mut bg);
    }
    let mut style = Style::default();
    if let Some(fg) = fg {
        style = style.fg(fg);
    }
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    if let Some(underline_color) = basic.style.underline_color.map(ghostty_cell_color) {
        style = style.underline_color(underline_color);
    }
    let mut modifiers = Modifier::empty();
    if basic.style.bold {
        modifiers |= Modifier::BOLD;
    }
    if basic.style.italic {
        modifiers |= Modifier::ITALIC;
    }
    if basic.style.faint {
        modifiers |= Modifier::DIM;
    }
    if basic.style.blink {
        modifiers |= Modifier::SLOW_BLINK;
    }
    if basic.style.underlined {
        modifiers |= Modifier::UNDERLINED;
    }
    if basic.style.strikethrough {
        modifiers |= Modifier::CROSSED_OUT;
    }
    style.add_modifier(modifiers)
}

fn ghostty_default_fg(
    color: crate::ghostty::RgbColor,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial: Option<crate::ghostty::RgbColor>,
) -> Option<Color> {
    if let Some(host) = host_theme.foreground {
        if host == terminal_theme_color(color) {
            None
        } else {
            Some(ghostty_color(color))
        }
    } else if initial.is_some_and(|i| i != color) {
        Some(ghostty_color(color))
    } else {
        None
    }
}

fn ghostty_default_bg(
    color: crate::ghostty::RgbColor,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial: Option<crate::ghostty::RgbColor>,
) -> Option<Color> {
    if let Some(host) = host_theme.background {
        if host == terminal_theme_color(color) {
            None
        } else {
            Some(ghostty_color(color))
        }
    } else if initial.is_some_and(|i| i != color) {
        Some(ghostty_color(color))
    } else {
        None
    }
}

fn terminal_theme_color(color: crate::ghostty::RgbColor) -> crate::terminal_theme::RgbColor {
    crate::terminal_theme::RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn ghostty_cell_color(color: crate::ghostty::CellColor) -> Color {
    match color {
        crate::ghostty::CellColor::Palette(index) => Color::Indexed(index),
        crate::ghostty::CellColor::Rgb(color) => ghostty_color(color),
    }
}

fn ghostty_color(color: crate::ghostty::RgbColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}
