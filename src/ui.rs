use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{theme::Theme, theme::THEMES, utils::LOGIN_SHELL_SENTINEL};

#[derive(Clone, Copy)]
pub(crate) struct AgentPreset {
    pub label: &'static str,
    pub command: &'static str,
    /// Binary token to look for in the pane's output when scraping a resume
    /// hint after the agent exits. `None` disables capture (e.g. plain shell).
    pub binary: Option<&'static str>,
}

pub(crate) const AGENT_PRESETS: [AgentPreset; 5] = [
    AgentPreset {
        label: "Terminal",
        command: LOGIN_SHELL_SENTINEL,
        binary: None,
    },
    AgentPreset {
        label: "Pi",
        command: "pi",
        binary: Some("pi"),
    },
    AgentPreset {
        label: "Cursor",
        command: "agent",
        binary: Some("agent"),
    },
    AgentPreset {
        label: "Codex",
        command: "codex",
        binary: Some("codex"),
    },
    AgentPreset {
        label: "OpenCode",
        command: "opencode",
        binary: Some("opencode"),
    },
];

/// Returns the binary token associated with a stored agent command, or `None`
/// for the plain login shell / unknown commands.
pub(crate) fn agent_binary_for_command(command: &str) -> Option<&'static str> {
    let normalized = match command {
        "cursor-agent" => "agent",
        "opencode-agent" => "opencode",
        other => other,
    };
    AGENT_PRESETS
        .iter()
        .find(|preset| preset.command == normalized)
        .and_then(|preset| preset.binary)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelSettingsFocus {
    Name,
    Agent,
    Confirm,
}

impl PanelSettingsFocus {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Agent,
            Self::Agent => Self::Confirm,
            Self::Confirm => Self::Name,
        }
    }

    pub(crate) fn prev(self) -> Self {
        match self {
            Self::Name => Self::Confirm,
            Self::Agent => Self::Name,
            Self::Confirm => Self::Agent,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Modal {
    Help,
    Theme,
    DefaultAgent {
        agent_index: usize,
    },
    PanelSettings {
        pane_id: usize,
        name: String,
        agent_index: usize,
        focus: PanelSettingsFocus,
    },
    CloseConfirm {
        pane_id: usize,
    },
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

pub(crate) fn help_debug_toggle_button_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: 22.min(area.width.saturating_sub(7)),
        height: 1,
    }
}

pub(crate) fn settings_button_area(size: Rect) -> Rect {
    Rect {
        x: size.x,
        y: size.y,
        width: 10.min(size.width),
        height: 1.min(size.height),
    }
}

pub(crate) fn render_settings_button(f: &mut ratatui::Frame<'_>, size: Rect, theme: Theme) {
    let area = settings_button_area(size);
    let button = Paragraph::new("[Settings]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.accent).bg(theme.background));
    f.render_widget(button, area);
}

pub(crate) fn default_agent_button_area(size: Rect, label: &str) -> Rect {
    let width = (2u16.saturating_add(label.chars().count() as u16))
        .min(size.width)
        .max(4);
    Rect {
        x: size.right().saturating_sub(width),
        y: size.y,
        width,
        height: 1.min(size.height),
    }
}

pub(crate) fn render_default_agent_button(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    label: &str,
    active: bool,
) {
    let area = default_agent_button_area(size, label);

    // "Default" caption rendered to the left of the button. Purely cosmetic;
    // the click hit area still maps to the button rect only.
    const PREFIX: &str = "Default ";
    let prefix_len = PREFIX.chars().count() as u16;
    if area.x >= size.x.saturating_add(prefix_len) {
        let prefix_area = Rect {
            x: area.x.saturating_sub(prefix_len),
            y: area.y,
            width: prefix_len,
            height: area.height,
        };
        f.render_widget(
            Paragraph::new(PREFIX)
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            prefix_area,
        );
    }

    let button = Paragraph::new(format!("[{label}]"))
        .alignment(Alignment::Center)
        .style(if active {
            Style::default().fg(theme.accent).bg(theme.background)
        } else {
            Style::default().fg(theme.foreground).bg(theme.background)
        });
    f.render_widget(button, area);
}

pub(crate) fn render_help_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    debug_containers: bool,
) {
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
    let close_button = Paragraph::new("🗙")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(close_button, close_area);

    let debug_area = help_debug_toggle_button_area(area);
    let debug_button = Paragraph::new(if debug_containers {
        "[Containers: ON]"
    } else {
        "[Containers: OFF]"
    })
    .alignment(Alignment::Left)
    .style(if debug_containers {
        Style::default().fg(theme.accent).bg(theme.background)
    } else {
        Style::default().fg(theme.muted).bg(theme.background)
    });
    f.render_widget(debug_button, debug_area);

    let theme_line = Paragraph::new(format!("Current theme: {}", theme.name))
        .style(Style::default().fg(theme.muted).bg(theme.background));
    f.render_widget(
        theme_line,
        Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        },
    );

    let shortcuts = Paragraph::new(
        "Ctrl+Space: Show/hide shortcuts\n\
T: Theme selector\n\
D: Toggle container debug boxes\n\
Ctrl+Q: Quit\n\
Ctrl+Alt+Arrows: Split pane\n\
Ctrl+Shift+A / B: Split right / down\n\
Ctrl+PgUp/PgDn: Cycle pane focus\n\
Ctrl+W: Close focused pane\n\
Ctrl+Arrows: Move focus\n\
Ctrl+Shift+Arrows: Resize pane edges\n\
Ctrl+Shift+K/J: Resize top/bottom (if Up/Down are captured by the OS)\n\
Mouse drag: Resize pane dividers\n\
Ctrl+Shift+M: Toggle mouse capture for terminal text selection\n\
Shift+PageUp/PageDown: Scroll by page\n\
Shift+Home/End: Scroll to top/bottom",
    )
    .style(Style::default().fg(theme.foreground).bg(theme.background));
    let shortcuts_area = Rect {
        x: inner.x,
        y: inner.y + 3,
        width: inner.width,
        height: inner.height.saturating_sub(3),
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
    lines.push(
        Line::from(vec![Span::raw("Up/Down: move  Enter: apply  Esc: back")])
            .style(Style::default().fg(theme.muted)),
    );

    let list = Paragraph::new(Text::from(lines))
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false });
    f.render_widget(list, inner);
}

pub(crate) fn panel_settings_modal_area(size: Rect) -> Rect {
    let desired_width = size.width.saturating_mul(60).saturating_div(100);
    let desired_height = size.height.saturating_mul(80).saturating_div(100);
    let width = if size.width < 40 {
        size.width
    } else {
        desired_width.max(40).min(size.width)
    };
    let height = if size.height < 16 {
        size.height
    } else {
        desired_height.max(16).min(size.height)
    };

    Rect {
        x: size.x + (size.width.saturating_sub(width)) / 2,
        y: size.y + (size.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// Content rectangle inside the modal frame (must match `render_panel_settings_modal`).
pub(crate) fn panel_settings_modal_inner(modal_area: Rect) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .title("Panel Settings")
        .inner(modal_area)
}

pub(crate) fn panel_settings_close_button_area(area: Rect) -> Rect {
    Rect {
        x: area.right().saturating_sub(5),
        y: area.y + 1,
        width: 3,
        height: 1,
    }
}

pub(crate) fn panel_settings_name_input_area(inner: Rect) -> Rect {
    Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: 3,
    }
}

pub(crate) fn panel_settings_agent_list_area(inner: Rect) -> Rect {
    Rect {
        x: inner.x,
        y: inner.y + 6,
        width: inner.width,
        height: 8,
    }
}

pub(crate) fn panel_settings_cancel_button_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 5,
        y: area.bottom().saturating_sub(2),
        width: 10,
        height: 1,
    }
}

pub(crate) fn panel_settings_confirm_button_area(area: Rect) -> Rect {
    Rect {
        x: area.right().saturating_sub(15),
        y: area.bottom().saturating_sub(2),
        width: 10,
        height: 1,
    }
}

pub(crate) fn default_agent_dropdown_area(size: Rect) -> Rect {
    let width = 18.min(size.width);
    let height = (AGENT_PRESETS.len() as u16 + 2).min(size.height.saturating_sub(1).max(1));
    Rect {
        x: size.right().saturating_sub(width),
        y: size.y.saturating_add(1),
        width,
        height,
    }
}

pub(crate) fn render_default_agent_dropdown(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    agent_index: usize,
) {
    let area = default_agent_dropdown_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agent")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, preset) in AGENT_PRESETS.iter().enumerate() {
        let selected = idx == agent_index;
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{}{}", marker, preset.label),
            style,
        )]));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

pub(crate) fn render_panel_settings_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    name: &str,
    agent_index: usize,
    focus: PanelSettingsFocus,
) {
    let area = panel_settings_modal_area(size);
    f.render_widget(Clear, area);

    let inner = panel_settings_modal_inner(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Panel Settings")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    f.render_widget(block, area);

    let close_area = panel_settings_close_button_area(area);
    let close_button = Paragraph::new("🗙")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(close_button, close_area);

    let hint = Paragraph::new("Tab: next field   Esc: cancel   Enter: confirm")
        .style(Style::default().fg(theme.muted).bg(theme.background));
    f.render_widget(
        hint,
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    let name_label =
        Paragraph::new("Name").style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(
        name_label,
        Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        },
    );

    let name_area = panel_settings_name_input_area(inner);
    let name_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(if focus == PanelSettingsFocus::Name {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.muted)
        });
    let name_inner = name_block.inner(name_area);
    f.render_widget(name_block, name_area);
    f.render_widget(
        Paragraph::new(name.to_string())
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        name_inner,
    );

    let agent_label =
        Paragraph::new("Agent").style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(
        agent_label,
        Rect {
            x: inner.x,
            y: inner.y + 5,
            width: inner.width,
            height: 1,
        },
    );

    let agent_area = panel_settings_agent_list_area(inner);
    let agent_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(if focus == PanelSettingsFocus::Agent {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.muted)
        });
    let agent_inner = agent_block.inner(agent_area);
    f.render_widget(agent_block, agent_area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, preset) in AGENT_PRESETS.iter().enumerate() {
        let selected = idx == agent_index;
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{}{}", marker, preset.label),
            style,
        )]));
    }
    lines.push(Line::from(""));
    lines.push(
        Line::from(vec![Span::raw("Up/Down: change agent")])
            .style(Style::default().fg(theme.muted)),
    );

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false }),
        agent_inner,
    );

    let cancel = Paragraph::new("[Cancel]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted).bg(theme.background));
    f.render_widget(cancel, panel_settings_cancel_button_area(area));

    let confirm = Paragraph::new("[Confirm]")
        .alignment(Alignment::Center)
        .style(if focus == PanelSettingsFocus::Confirm {
            Style::default().fg(theme.accent).bg(theme.background)
        } else {
            Style::default().fg(theme.foreground).bg(theme.background)
        });
    f.render_widget(confirm, panel_settings_confirm_button_area(area));

    if focus == PanelSettingsFocus::Name {
        let cursor_x = name_inner.x.saturating_add(
            name.chars()
                .count()
                .min(name_inner.width.saturating_sub(1) as usize) as u16,
        );
        f.set_cursor(cursor_x, name_inner.y);
    }
}

pub(crate) fn close_confirm_modal_area(size: Rect) -> Rect {
    let width = 40.min(size.width);
    let height = 7.min(size.height);
    Rect {
        x: size.x + (size.width.saturating_sub(width)) / 2,
        y: size.y + (size.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn close_confirm_cancel_button_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 5,
        y: area.bottom().saturating_sub(2),
        width: 10,
        height: 1,
    }
}

pub(crate) fn close_confirm_confirm_button_area(area: Rect) -> Rect {
    Rect {
        x: area.right().saturating_sub(15),
        y: area.bottom().saturating_sub(2),
        width: 10,
        height: 1,
    }
}

pub(crate) fn render_close_confirm_modal(f: &mut ratatui::Frame<'_>, size: Rect, theme: Theme) {
    let area = close_confirm_modal_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Close Pane")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let message = Paragraph::new("Are you sure you want to close this pane?")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(theme.background));
    f.render_widget(
        message,
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 2,
        },
    );

    let cancel = Paragraph::new("[Cancel]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted).bg(theme.background));
    f.render_widget(cancel, close_confirm_cancel_button_area(area));

    let confirm = Paragraph::new("[Confirm]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.accent).bg(theme.background));
    f.render_widget(confirm, close_confirm_confirm_button_area(area));
}
