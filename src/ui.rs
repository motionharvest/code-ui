use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{theme::Theme, theme::THEMES};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Modal {
    Help,
    Theme,
    Rename { pane_id: usize, input: String },
}

pub(crate) fn help_modal_area(size: Rect) -> Rect {
    let desired_width = size.width.saturating_mul(70).saturating_div(100);
    let desired_height = size.height.saturating_mul(70).saturating_div(100);
    let width = if size.width < 40 {
        size.width
    } else {
        desired_width.max(40).min(size.width)
    };
    let height = if size.height < 12 {
        size.height
    } else {
        desired_height.max(12).min(size.height)
    };

    Rect {
        x: size.x + (size.width.saturating_sub(width)) / 2,
        y: size.y + (size.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn help_close_button_area(area: Rect) -> Rect {
    Rect {
        x: area.right().saturating_sub(5),
        y: area.y + 1,
        width: 3,
        height: 1,
    }
}

pub(crate) fn render_help_modal(f: &mut ratatui::Frame<'_>, size: Rect, theme: Theme) {
    let area = help_modal_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Shortcuts")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let close_area = help_close_button_area(area);
    let close_button = Paragraph::new("[X]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(close_button, close_area);

    let theme_line = Paragraph::new(format!("Current theme: {}", theme.name))
        .style(Style::default().fg(theme.muted).bg(theme.background));
    f.render_widget(
        theme_line,
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    let shortcuts = Paragraph::new(
        "Ctrl+Space: Show/hide shortcuts\n\
T: Theme selector\n\
Ctrl+R: Reload\n\
Ctrl+Q: Quit\n\
Ctrl+Alt+Arrows: Split pane\n\
Ctrl+Alt+- / _: Close focused pane\n\
Ctrl+Arrows: Move focus\n\
Shift+PageUp/PageDown: Scroll by page\n\
Shift+Home/End: Scroll to top/bottom",
    )
    .style(Style::default().fg(theme.foreground).bg(theme.background));
    let shortcuts_area = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };
    f.render_widget(shortcuts, shortcuts_area);
}

pub(crate) fn render_theme_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    preview_index: usize,
    theme: Theme,
) {
    let area = help_modal_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Themes")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Select a theme:",
        Style::default().fg(theme.foreground),
    )]));
    lines.push(Line::from(""));

    for (idx, preset) in THEMES.iter().enumerate() {
        let selected = idx == preview_index;
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(preset.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{}{}", marker, preset.name),
            style,
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw("Up/Down: move  Enter: apply  Esc: back")]).style(Style::default().fg(theme.muted)));

    let list = Paragraph::new(Text::from(lines))
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false });
    f.render_widget(list, inner);
}

pub(crate) fn rename_modal_area(size: Rect) -> Rect {
    let desired_width = size.width.saturating_mul(50).saturating_div(100);
    let width = if size.width < 30 {
        size.width
    } else {
        desired_width.max(30).min(size.width)
    };
    let height = if size.height < 7 { size.height } else { 7.min(size.height) };

    Rect {
        x: size.x + (size.width.saturating_sub(width)) / 2,
        y: size.y + (size.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn render_rename_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    input: &str,
) {
    let area = rename_modal_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Rename Pane")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let close_area = rename_close_button_area(area);
    let close_button = Paragraph::new("[X]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(close_button, close_area);

    let hint = Paragraph::new("Enter a new name:\nEsc: cancel   Enter: save   X: close pane")
        .style(Style::default().fg(theme.muted).bg(theme.background));
    let hint_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };
    f.render_widget(hint, hint_area);

    let input_area = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: 3,
    };
    let input_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let input_inner = input_block.inner(input_area);
    f.render_widget(input_block, input_area);
    f.render_widget(
        Paragraph::new(input.to_string())
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        input_inner,
    );

    let cursor_x = input_inner.x.saturating_add(
        input
            .chars()
            .count()
            .min(input_inner.width.saturating_sub(1) as usize) as u16,
    );
    f.set_cursor(cursor_x, input_inner.y);
}

pub(crate) fn rename_close_button_area(area: Rect) -> Rect {
    Rect {
        x: area.right().saturating_sub(5),
        y: area.y + 1,
        width: 3,
        height: 1,
    }
}
