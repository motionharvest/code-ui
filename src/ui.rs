use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    theme::Theme,
    theme::THEMES,
    utils::{contains, LOGIN_SHELL_SENTINEL},
};

pub(crate) const COMMANDER_COMMAND: &str = "commander";
pub(crate) const TOP_CHROME_ROWS: u16 = 3;
pub(crate) const WORKSPACE_SIDEBAR_WIDTH: u16 = 24;
const APP_NAME: &str = "Code UI";
const APP_VERSION: &str = "0.0.2";
const APP_HANDLE: &str = "@motionharvest";
const WORKSPACE_ENTRY_MARGIN_X: u16 = 1;
const WORKSPACE_ENTRY_MARGIN_TOP: u16 = 0;
const WORKSPACE_ENTRY_HEIGHT: u16 = 3;
const WORKSPACE_ENTRY_GAP: u16 = 0;
const COMMANDER_HEIGHT_MULTIPLIER: u16 = 5;

/// Truncate to a maximum number of terminal cells (counted as Unicode scalar
/// values, matching ratatui's default monospace assumption).
pub(crate) fn truncate_to_width(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }

    if width == 0 {
        String::new()
    } else if width == 1 {
        "…".to_string()
    } else {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

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
        label: "Opencode",
        command: "opencode",
        binary: Some("opencode"),
    },
];

pub(crate) fn default_agent_index() -> usize {
    AGENT_PRESETS
        .iter()
        .position(|preset| preset.command == "pi")
        .unwrap_or(0)
}

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
    NewPanePicker {
        pane_id: usize,
        source_pane_id: usize,
        close_on_cancel: bool,
        name: String,
        name_error: Option<String>,
        cursor: usize,
        name_selected: bool,
        agent_index: usize,
    },
    #[allow(dead_code)]
    PanelSettings {
        pane_id: usize,
        name: String,
        name_error: Option<String>,
        agent_index: usize,
        focus: PanelSettingsFocus,
    },
    WorkspaceSettings {
        workspace_index: usize,
        name: String,
        name_error: Option<String>,
        cursor: usize,
        action_index: usize,
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

pub(crate) fn workspace_item_area(sidebar_area: Rect, index: usize) -> Rect {
    let first_workspace_y = commander_item_area(sidebar_area)
        .bottom()
        .saturating_add(WORKSPACE_ENTRY_GAP);
    let y = first_workspace_y.saturating_add(
        (index as u16).saturating_mul(WORKSPACE_ENTRY_HEIGHT.saturating_add(WORKSPACE_ENTRY_GAP)),
    );
    Rect {
        x: sidebar_area.x.saturating_add(WORKSPACE_ENTRY_MARGIN_X),
        y,
        width: sidebar_area
            .width
            .saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_mul(2)),
        height: WORKSPACE_ENTRY_HEIGHT.min(
            sidebar_area
                .height
                .saturating_sub(y.saturating_sub(sidebar_area.y)),
        ),
    }
}

fn sidebar_slot_area(sidebar_area: Rect, slot_index: usize) -> Rect {
    let y = sidebar_area
        .y
        .saturating_add(WORKSPACE_ENTRY_MARGIN_TOP)
        .saturating_add(
            (slot_index as u16)
                .saturating_mul(WORKSPACE_ENTRY_HEIGHT.saturating_add(WORKSPACE_ENTRY_GAP)),
        );
    Rect {
        x: sidebar_area.x.saturating_add(WORKSPACE_ENTRY_MARGIN_X),
        y,
        width: sidebar_area
            .width
            .saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_mul(2)),
        height: WORKSPACE_ENTRY_HEIGHT.min(
            sidebar_area
                .height
                .saturating_sub(y.saturating_sub(sidebar_area.y)),
        ),
    }
}

pub(crate) fn commander_item_area(sidebar_area: Rect) -> Rect {
    let mut area = sidebar_slot_area(sidebar_area, 0);
    let desired = WORKSPACE_ENTRY_HEIGHT.saturating_mul(COMMANDER_HEIGHT_MULTIPLIER);
    area.height = desired.min(
        sidebar_area
            .height
            .saturating_sub(WORKSPACE_ENTRY_MARGIN_TOP),
    );
    area
}

pub(crate) fn workspace_add_button_area(sidebar_area: Rect, workspace_count: usize) -> Rect {
    workspace_item_area(sidebar_area, workspace_count)
}

pub(crate) fn workspace_menu_button_area(sidebar_area: Rect, index: usize) -> Rect {
    let item = workspace_item_area(sidebar_area, index);
    Rect {
        x: item.right().saturating_sub(3),
        y: item.y.saturating_add(1),
        width: 1.min(item.width.saturating_sub(2)),
        height: 1.min(item.height.saturating_sub(2)),
    }
}

pub(crate) fn workspace_hit_index(
    sidebar_area: Rect,
    workspace_count: usize,
    x: u16,
    y: u16,
) -> Option<usize> {
    (0..workspace_count).find(|idx| contains(workspace_item_area(sidebar_area, *idx), x, y))
}

pub(crate) fn workspace_menu_hit_index(
    sidebar_area: Rect,
    workspace_count: usize,
    x: u16,
    y: u16,
) -> Option<usize> {
    (0..workspace_count).find(|idx| contains(workspace_menu_button_area(sidebar_area, *idx), x, y))
}

pub(crate) fn workspace_add_button_hit(
    sidebar_area: Rect,
    workspace_count: usize,
    x: u16,
    y: u16,
) -> bool {
    contains(
        workspace_add_button_area(sidebar_area, workspace_count),
        x,
        y,
    )
}

pub(crate) fn commander_button_hit(sidebar_area: Rect, x: u16, y: u16) -> bool {
    contains(commander_item_area(sidebar_area), x, y)
}

pub(crate) fn render_workspace_sidebar(
    f: &mut ratatui::Frame<'_>,
    sidebar_area: Rect,
    theme: Theme,
    workspace_names: &[String],
    active_workspace_index: usize,
    commander_selected: bool,
    sidebar_workspace_selected_index: Option<usize>,
    sidebar_add_button_selected: bool,
) {
    if sidebar_area.width == 0 || sidebar_area.height == 0 {
        return;
    }

    f.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        sidebar_area,
    );

    let commander_area = commander_item_area(sidebar_area);
    if commander_area.width >= 3 && commander_area.height >= 3 {
        let commander_style = if commander_selected {
            Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(theme.background)
        };
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(commander_style)
                .style(Style::default().bg(theme.background)),
            commander_area,
        );
        f.render_widget(
            Paragraph::new("Commander")
                .alignment(Alignment::Center)
                .style(commander_style),
            Rect {
                x: commander_area.x.saturating_add(1),
                y: commander_area.y.saturating_add(1),
                width: commander_area.width.saturating_sub(2),
                height: 1,
            },
        );
    }

    for (idx, name) in workspace_names.iter().enumerate() {
        let item_area = workspace_item_area(sidebar_area, idx);
        if item_area.width < 3 || item_area.height < 3 {
            continue;
        }
        let active = idx == active_workspace_index;
        let keyboard_selected = sidebar_workspace_selected_index == Some(idx);
        let border_style = if active || keyboard_selected {
            Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(theme.background)
        };
        let border_type = if active && keyboard_selected {
            BorderType::Double
        } else if keyboard_selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        };
        let label_width = item_area.width.saturating_sub(5) as usize;
        let label = truncate_to_width(name, label_width);
        let menu_area = workspace_menu_button_area(sidebar_area, idx);
        let label_area = Rect {
            x: item_area.x.saturating_add(1),
            y: item_area.y.saturating_add(1),
            width: item_area.width.saturating_sub(4),
            height: 1,
        };

        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .style(Style::default().bg(theme.background)),
            item_area,
        );
        f.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(border_style),
            label_area,
        );

        if menu_area.width > 0 && menu_area.height > 0 {
            f.render_widget(
                Paragraph::new("⋮")
                    .alignment(Alignment::Center)
                    .style(border_style),
                menu_area,
            );
        }
    }

    let add_area = workspace_add_button_area(sidebar_area, workspace_names.len());
    if add_area.width >= 3 && add_area.height >= 3 {
        let style = if sidebar_add_button_selected {
            Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(theme.background)
        };
        let border_type = if sidebar_add_button_selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        };
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(style)
                .style(Style::default().bg(theme.background)),
            add_area,
        );
        f.render_widget(
            Paragraph::new("+")
                .alignment(Alignment::Center)
                .style(style),
            Rect {
                x: add_area.x.saturating_add(1),
                y: add_area.y.saturating_add(1),
                width: add_area.width.saturating_sub(2),
                height: 1,
            },
        );
    }
}

pub(crate) fn render_top_chrome(f: &mut ratatui::Frame<'_>, size: Rect, theme: Theme) {
    if size.width == 0 || size.height < 2 {
        return;
    }

    let header_row = Rect {
        x: size.x,
        y: size.y.saturating_add(1),
        width: size.width,
        height: 1,
    };
    if header_row.y >= size.bottom() {
        return;
    }

    let handle_width = APP_HANDLE.chars().count() as u16;
    let handle_area = Rect {
        x: header_row
            .x
            .saturating_add(header_row.width.saturating_sub(handle_width)),
        y: header_row.y,
        width: handle_width.min(header_row.width),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(APP_HANDLE)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        handle_area,
    );

    let left_width = header_row
        .width
        .saturating_sub(handle_area.width.saturating_add(1));
    if left_width == 0 {
        return;
    }

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            APP_NAME,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(APP_VERSION, Style::default().fg(theme.muted)),
    ]))
    .alignment(Alignment::Left)
    .style(Style::default().bg(theme.background));
    f.render_widget(
        title,
        Rect {
            x: header_row
                .x
                .saturating_add(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
            y: header_row.y,
            width: left_width.saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
            height: 1,
        },
    );
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
Ctrl+Alt+Arrows: Split pane (pick agent)\n\
Ctrl+Shift+A / B: Split right / down\n\
Ctrl+PgUp/PgDn: Cycle pane focus\n\
Ctrl+W: Close focused pane\n\
Ctrl+Arrows: Move focus (panes/sidebar)\n\
Ctrl+Shift+Arrows: Resize pane edges\n\
Ctrl+Shift+K/J: Resize top/bottom (if Up/Down are captured by the OS)\n\
Mouse drag: Resize pane dividers\n\
Drag pane title onto another pane: Swap pane positions\n\
Drag pane contents: Select text\n\
Shift+Left/Right: Adjust selection by character\n\
Ctrl+C: Copy selection, or interrupt if nothing is selected\n\
Ctrl+Shift+M: Toggle mouse capture\n\
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

pub(crate) fn new_pane_picker_modal_area(container: Rect) -> Rect {
    let width = 26.min(container.width);
    let height = (AGENT_PRESETS.len() as u16 + 9).min(container.height);
    Rect {
        x: container.x + (container.width.saturating_sub(width)) / 2,
        y: container.y + (container.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn new_pane_picker_name_input_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: 3,
    }
}

pub(crate) fn new_pane_picker_list_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        x: inner.x,
        y: inner.y + 6,
        width: inner.width,
        height: AGENT_PRESETS.len() as u16,
    }
}

pub(crate) fn render_new_pane_picker_modal(
    f: &mut ratatui::Frame<'_>,
    container: Rect,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    cursor: usize,
    name_selected: bool,
    agent_index: usize,
    agent_available: &[bool],
) {
    let area = new_pane_picker_modal_area(container);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title("─ New Pane ─")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let hint = Paragraph::new("Type name, L/R cursor, U/D agent")
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

    let name_area = new_pane_picker_name_input_area(area);
    let name_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(if name_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(theme.accent)
        });
    let name_inner = name_block.inner(name_area);
    f.render_widget(name_block, name_area);
    f.render_widget(
        Paragraph::new(name.to_string()).style(if name_selected {
            Style::default().fg(theme.background).bg(theme.accent)
        } else {
            Style::default().fg(theme.foreground).bg(theme.background)
        }),
        name_inner,
    );
    if let Some(error) = name_error {
        let error_area = Rect {
            x: inner.x,
            y: name_area.bottom(),
            width: inner.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red).bg(theme.background)),
            error_area,
        );
    }

    let list_area = new_pane_picker_list_area(area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, preset) in AGENT_PRESETS.iter().enumerate() {
        let available = agent_available.get(idx).copied().unwrap_or(true);
        let selected = idx == agent_index && available;
        let marker = if selected {
            "> "
        } else if available {
            "  "
        } else {
            "x "
        };
        let style = if !available {
            Style::default().fg(theme.muted)
        } else if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        let suffix = if available { "" } else { " (in use)" };
        lines.push(Line::from(vec![Span::styled(
            format!("{}{}{}", marker, preset.label, suffix),
            style,
        )]));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false }),
        list_area,
    );

    let clamped_cursor = if name_selected {
        name.chars().count()
    } else {
        cursor.min(name.chars().count())
    };
    let cursor_x = name_inner
        .x
        .saturating_add(clamped_cursor.min(name_inner.width.saturating_sub(1) as usize) as u16);
    f.set_cursor(cursor_x, name_inner.y);
}

pub(crate) fn render_panel_settings_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    agent_index: usize,
    focus: PanelSettingsFocus,
    agent_available: &[bool],
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
        .border_style(if name_error.is_some() {
            Style::default().fg(Color::Red)
        } else if focus == PanelSettingsFocus::Name {
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
    if let Some(error) = name_error {
        f.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red).bg(theme.background)),
            Rect {
                x: inner.x,
                y: name_area.bottom(),
                width: inner.width,
                height: 1,
            },
        );
    }

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
        let available = agent_available.get(idx).copied().unwrap_or(true);
        let selected = idx == agent_index && available;
        let marker = if selected {
            "> "
        } else if available {
            "  "
        } else {
            "x "
        };
        let style = if !available {
            Style::default().fg(theme.muted)
        } else if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        let suffix = if available { "" } else { " (in use)" };
        lines.push(Line::from(vec![Span::styled(
            format!("{}{}{}", marker, preset.label, suffix),
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

pub(crate) fn workspace_settings_modal_area(size: Rect) -> Rect {
    let width = 46.min(size.width);
    let height = 11.min(size.height);
    Rect {
        x: size.x + (size.width.saturating_sub(width)) / 2,
        y: size.y + (size.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub(crate) fn workspace_settings_name_input_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: 3,
    }
}

pub(crate) fn workspace_settings_action_list_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        x: inner.x,
        y: inner.y + 6,
        width: inner.width,
        height: 3,
    }
}

pub(crate) fn workspace_settings_action_hit_index(area: Rect, x: u16, y: u16) -> Option<usize> {
    let action_area = workspace_settings_action_list_area(area);
    if !contains(action_area, x, y) {
        return None;
    }
    Some((y.saturating_sub(action_area.y) as usize).min(2))
}

fn workspace_settings_action_style(theme: Theme, action_index: usize, selected: bool) -> Style {
    let mut style = if action_index == 2 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(theme.foreground)
    };
    if selected {
        style = style.add_modifier(Modifier::BOLD);
        if action_index != 2 {
            style = style.fg(theme.accent);
        }
    }
    style
}

pub(crate) fn render_workspace_settings_modal(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    cursor: usize,
    action_index: usize,
) {
    let area = workspace_settings_modal_area(size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title("─ Workspace ─")
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let hint = Paragraph::new("Type name, L/R cursor, U/D action")
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

    let name_area = workspace_settings_name_input_area(area);
    let name_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .border_style(if name_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(theme.accent)
        });
    let name_inner = name_block.inner(name_area);
    f.render_widget(name_block, name_area);
    f.render_widget(
        Paragraph::new(name.to_string())
            .style(Style::default().fg(theme.foreground).bg(theme.background)),
        name_inner,
    );

    if let Some(error) = name_error {
        f.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red).bg(theme.background)),
            Rect {
                x: inner.x,
                y: name_area.bottom(),
                width: inner.width,
                height: 1,
            },
        );
    }

    let actions_area = workspace_settings_action_list_area(area);
    let actions = ["Save", "Cancel", "Close Workspace"];
    let mut action_lines = Vec::new();
    for (idx, label) in actions.iter().enumerate() {
        let selected = idx == action_index.min(2);
        let marker = if selected { "> " } else { "  " };
        action_lines.push(Line::from(vec![Span::styled(
            format!("{}{}", marker, label),
            workspace_settings_action_style(theme, idx, selected),
        )]));
    }
    f.render_widget(
        Paragraph::new(Text::from(action_lines))
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false }),
        actions_area,
    );

    let cursor_x = name_inner
        .x
        .saturating_add(cursor.min(name_inner.width.saturating_sub(1) as usize) as u16);
    f.set_cursor(cursor_x, name_inner.y);
}
