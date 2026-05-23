use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    layout::pane_combobox_dropdown_area,
    theme::Theme,
    theme::THEMES,
    utils::{contains, LOGIN_SHELL_SENTINEL},
};

pub(crate) fn pane_chrome_title_label(pane_title: &str, command: &str) -> String {
    format!("{} [{}] ▼", pane_title, agent_label_for_command(command))
}

pub(crate) fn commander_chrome_title_label() -> String {
    "Commander [harness]".to_string()
}

/// Two-row title bar chrome shared by workspace panes and the Commander panel.
pub(crate) fn render_panel_title_chrome(
    f: &mut ratatui::Frame<'_>,
    panel_area: Rect,
    title: &str,
    subtitle: &str,
    chrome_style: Style,
    theme: Theme,
    show_window_controls: bool,
    is_maximized: bool,
) {
    use crate::layout::{
        pane_title_bar_area, pane_title_chrome_reserve, pane_title_y, PANE_TITLE_LEFT_PADDING,
    };

    let title_bar = pane_title_bar_area(panel_area);
    let title_y = pane_title_y(panel_area);
    if title_y >= panel_area.bottom() {
        return;
    }

    let title_bar_width = title_bar.width;
    if title_bar_width <= PANE_TITLE_LEFT_PADDING {
        return;
    }

    let chrome_reserve = if show_window_controls {
        pane_title_chrome_reserve(panel_area.width)
    } else {
        0
    };
    let title_max = title_bar_width
        .saturating_sub(chrome_reserve)
        .saturating_sub(PANE_TITLE_LEFT_PADDING + 7)
        as usize;
    let title_slot_w = title_bar_width;
    let title_body = truncate_to_width(title, title_max);
    let usage_max = title_slot_w.saturating_sub(7) as usize;
    let usage_body = truncate_to_width(subtitle, usage_max);
    let top_label_prefix = format!("╭─┐ {}", title_body);
    let bottom_prefix = "│ └ ";
    let bottom_after_usage = " ";
    let top_corner_col = top_label_prefix.chars().count() + 2;
    let bottom_corner_col = bottom_prefix.chars().count()
        + usage_body.chars().count()
        + bottom_after_usage.chars().count()
        + 1;
    let top_corner_padding = bottom_corner_col.saturating_sub(top_corner_col);
    let top_prefix = format!("{top_label_prefix}{} ┌", " ".repeat(top_corner_padding));
    let top_min_len = top_prefix.chars().count() + 2;
    let bottom_min_len = bottom_prefix.chars().count()
        + usage_body.chars().count()
        + bottom_after_usage.chars().count()
        + 1;
    let target_len = top_min_len.max(bottom_min_len);
    let top_dash_count = target_len.saturating_sub(top_prefix.chars().count());
    let bottom_dash_count = target_len.saturating_sub(bottom_min_len + 2);
    let title_text = format!("{top_prefix}{}", "─".repeat(top_dash_count));
    f.render_widget(
        Paragraph::new(title_text)
            .alignment(Alignment::Left)
            .style(chrome_style),
        Rect {
            x: title_bar.x,
            y: title_y,
            width: title_slot_w,
            height: 1,
        },
    );

    if title_y.saturating_add(1) < panel_area.bottom() {
        let usage_line = Line::from(vec![
            Span::styled(bottom_prefix, chrome_style),
            Span::styled(
                usage_body,
                Style::default().fg(theme.muted).bg(theme.background),
            ),
            Span::styled(
                format!(
                    "{bottom_after_usage}{}┘",
                    "─".repeat(bottom_dash_count)
                ),
                chrome_style,
            ),
        ]);
        f.render_widget(
            Paragraph::new(usage_line)
                .alignment(Alignment::Left)
                .style(chrome_style),
            Rect {
                x: title_bar.x,
                y: title_y.saturating_add(1),
                width: title_slot_w,
                height: 1,
            },
        );
    }

    if !show_window_controls {
        return;
    }

    let maximize_icon = if is_maximized { "🗗" } else { "⛶" };
    let controls_top = format!("─┐ {maximize_icon}  🗙 ┌─╮");
    let controls_bottom = " └──────┘ │";
    let controls_width = controls_top.chars().count() as u16;
    if panel_area.width >= controls_width {
        let controls_x = panel_area.right().saturating_sub(controls_width);
        f.render_widget(
            Paragraph::new(controls_top).style(chrome_style),
            Rect {
                x: controls_x,
                y: title_y,
                width: controls_width,
                height: 1,
            },
        );
        if title_y.saturating_add(1) < panel_area.bottom() {
            f.render_widget(
                Paragraph::new(controls_bottom).style(chrome_style),
                Rect {
                    x: controls_x,
                    y: title_y.saturating_add(1),
                    width: controls_width,
                    height: 1,
                },
            );
        }
    }
}

pub(crate) const COMMANDER_COMMAND: &str = "commander";
pub(crate) const TOP_CHROME_ROWS: u16 = 2;
pub(crate) const WORKSPACE_SIDEBAR_WIDTH: u16 = 47;
pub(crate) const WORKSPACE_BAR_HEIGHT: u16 = 2;
const APP_NAME: &str = "Code UI";
const APP_VERSION: &str = "0.0.2";
const APP_HANDLE: &str = "@motionharvest";
const WORKSPACE_ENTRY_MARGIN_X: u16 = 1;
const WORKSPACE_ENTRY_MARGIN_TOP: u16 = 0;
const WORKSPACE_ENTRY_HEIGHT: u16 = 1;
const WORKSPACE_ENTRY_GAP: u16 = 1;
const WORKSPACE_TAB_SIDE_PADDING: u16 = 1;
const WORKSPACE_TAB_TITLE_MENU_GAP: u16 = 1;
const WORKSPACE_TAB_MENU_COLUMNS: u16 = 1;
const WORKSPACE_TAB_ADD_BUTTON_WIDTH: u16 = 3;

fn workspace_tab_chrome_columns(show_menu: bool) -> u16 {
    let mut chrome = WORKSPACE_TAB_SIDE_PADDING.saturating_mul(2);
    if show_menu {
        chrome = chrome
            .saturating_add(WORKSPACE_TAB_TITLE_MENU_GAP)
            .saturating_add(WORKSPACE_TAB_MENU_COLUMNS);
    }
    chrome
}

fn workspace_tab_shows_menu(index: usize, active_workspace_index: usize) -> bool {
    index == active_workspace_index
}
const COMMANDER_TAB_WIDTH: u16 = WORKSPACE_SIDEBAR_WIDTH;

fn commander_panel_width(sidebar_area: Rect) -> u16 {
    COMMANDER_TAB_WIDTH.min(sidebar_area.width.saturating_sub(1))
}

fn color_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((205, 49, 49)),
        Color::Green => Some((13, 188, 121)),
        Color::Yellow => Some((229, 229, 16)),
        Color::Blue => Some((36, 114, 200)),
        Color::Magenta => Some((188, 63, 188)),
        Color::Cyan => Some((17, 168, 205)),
        Color::Gray => Some((229, 229, 229)),
        Color::DarkGray => Some((102, 102, 102)),
        Color::LightRed => Some((241, 76, 76)),
        Color::LightGreen => Some((35, 209, 139)),
        Color::LightYellow => Some((245, 245, 67)),
        Color::LightBlue => Some((59, 142, 234)),
        Color::LightMagenta => Some((214, 112, 214)),
        Color::LightCyan => Some((41, 184, 219)),
        Color::White => Some((255, 255, 255)),
        Color::Indexed(_) | Color::Reset => None,
    }
}

fn color_contrast_delta(a: Color, b: Color) -> Option<u16> {
    let (ar, ag, ab) = color_to_rgb(a)?;
    let (br, bg, bb) = color_to_rgb(b)?;
    Some(ar.abs_diff(br) as u16 + ag.abs_diff(bg) as u16 + ab.abs_diff(bb) as u16)
}

fn selected_tab_bg(theme: Theme) -> Color {
    let candidate = theme.palette.get(14).copied().unwrap_or(theme.accent);
    match color_contrast_delta(candidate, theme.background) {
        Some(delta) if delta >= 180 => candidate,
        _ => theme.accent,
    }
}

fn selected_tab_style(theme: Theme) -> Style {
    Style::default()
        .fg(theme.background)
        .bg(selected_tab_bg(theme))
        .add_modifier(Modifier::BOLD)
}

fn inactive_workspace_tab_style(theme: Theme, keyboard_focused: bool) -> Style {
    let mut style = Style::default().fg(theme.accent).bg(theme.background);
    if keyboard_focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn workspace_tab_rule_style(theme: Theme) -> Style {
    Style::default().fg(theme.muted).bg(theme.background)
}

fn workspace_tabs_start_x(sidebar_area: Rect) -> u16 {
    sidebar_area
        .x
        .saturating_add(commander_panel_width(sidebar_area))
        .saturating_add(1)
        .min(sidebar_area.right())
}

pub(crate) fn workspace_tab_width(name: &str, show_menu: bool) -> u16 {
    workspace_tab_chrome_columns(show_menu).saturating_add(name.chars().count() as u16)
}

fn workspace_tab_title_columns(tab_width: u16, show_menu: bool) -> u16 {
    tab_width.saturating_sub(workspace_tab_chrome_columns(show_menu))
}

fn workspace_item_x(
    sidebar_area: Rect,
    workspace_names: &[String],
    index: usize,
    active_workspace_index: usize,
) -> u16 {
    let mut x = workspace_tabs_start_x(sidebar_area);
    for (idx, name) in workspace_names.iter().take(index).enumerate() {
        let show_menu = workspace_tab_shows_menu(idx, active_workspace_index);
        x = x
            .saturating_add(workspace_tab_width(name, show_menu))
            .saturating_add(WORKSPACE_ENTRY_GAP);
    }
    x
}

fn workspace_tab_divider_columns(
    sidebar_area: Rect,
    workspace_names: &[String],
    active_workspace_index: usize,
) -> Vec<u16> {
    let mut xs = Vec::new();
    for idx in 0..workspace_names.len().saturating_sub(1) {
        let left = workspace_item_area(sidebar_area, workspace_names, idx, active_workspace_index);
        let right =
            workspace_item_area(sidebar_area, workspace_names, idx + 1, active_workspace_index);
        let gap_x = left.right();
        if right.x > gap_x {
            xs.push(gap_x);
        }
    }
    xs
}

fn active_tab_left_corner_x(active_tab: Rect, active_is_leftmost: bool) -> Option<u16> {
    if active_is_leftmost {
        None
    } else {
        Some(active_tab.x.saturating_sub(1))
    }
}

fn workspace_tab_rule_char(
    x: u16,
    active_tab: Rect,
    active_is_leftmost: bool,
    active_is_rightmost: bool,
    tab_dividers: &[u16],
) -> char {
    if active_tab_left_corner_x(active_tab, active_is_leftmost) == Some(x) {
        return '┘';
    }
    if !active_is_rightmost && x == active_tab.right() {
        return '└';
    }
    if x >= active_tab.x && x < active_tab.right() {
        return '▀';
    }
    if tab_dividers.contains(&x) {
        return '┴';
    }
    '─'
}

fn workspace_tab_rule_span_style(ch: char, theme: Theme) -> Style {
    if ch == '▀' {
        Style::default()
            .fg(selected_tab_bg(theme))
            .bg(theme.background)
    } else {
        workspace_tab_rule_style(theme)
    }
}

fn render_workspace_tab_chrome(
    f: &mut ratatui::Frame<'_>,
    sidebar_area: Rect,
    theme: Theme,
    workspace_names: &[String],
    active_workspace_index: usize,
    divider_x: u16,
) {
    if sidebar_area.height < 2 || workspace_names.is_empty() {
        return;
    }

    let rule_y = sidebar_area.bottom().saturating_sub(1);
    let active_tab =
        workspace_item_area(sidebar_area, workspace_names, active_workspace_index, active_workspace_index);
    let tab_row_style = Style::default().fg(theme.muted).bg(theme.background);

    if divider_x >= sidebar_area.x && divider_x < sidebar_area.right() {
        for y in [sidebar_area.y, rule_y] {
            f.render_widget(
                Paragraph::new("│")
                    .alignment(Alignment::Left)
                    .style(tab_row_style),
                Rect {
                    x: divider_x,
                    y,
                    width: 1,
                    height: 1,
                },
            );
        }
    }

    for idx in 0..workspace_names.len().saturating_sub(1) {
        let left = workspace_item_area(sidebar_area, workspace_names, idx, active_workspace_index);
        let right =
            workspace_item_area(sidebar_area, workspace_names, idx + 1, active_workspace_index);
        let gap_x = left.right();
        let gap_width = right.x.saturating_sub(gap_x);
        if gap_width == 0 {
            continue;
        }
        f.render_widget(
            Paragraph::new("│")
                .alignment(Alignment::Left)
                .style(tab_row_style),
            Rect {
                x: gap_x,
                y: sidebar_area.y,
                width: 1.min(gap_width),
                height: 1,
            },
        );
    }

    let first_workspace_x =
        workspace_item_area(sidebar_area, workspace_names, 0, active_workspace_index).x;
    let rule_width = sidebar_area.right().saturating_sub(first_workspace_x);
    if rule_width == 0 {
        return;
    }

    let tab_dividers =
        workspace_tab_divider_columns(sidebar_area, workspace_names, active_workspace_index);
    let active_is_leftmost = active_workspace_index == 0;
    let active_is_rightmost = active_workspace_index + 1 == workspace_names.len();
    let mut spans = Vec::with_capacity(rule_width as usize);
    for offset in 0..rule_width {
        let x = first_workspace_x.saturating_add(offset);
        let ch = workspace_tab_rule_char(
            x,
            active_tab,
            active_is_leftmost,
            active_is_rightmost,
            &tab_dividers,
        );
        let style = workspace_tab_rule_span_style(ch, theme);
        spans.push(Span::styled(ch.to_string(), style));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: first_workspace_x,
            y: rule_y,
            width: rule_width,
            height: 1,
        },
    );
}

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

pub(crate) fn agent_label_for_command(command: &str) -> &'static str {
    let normalized = match command {
        "cursor-agent" => "agent",
        "opencode-agent" => "opencode",
        other => other,
    };
    AGENT_PRESETS
        .iter()
        .find(|preset| preset.command == normalized)
        .map(|preset| preset.label)
        .unwrap_or("Terminal")
}

pub(crate) fn agent_command_for_input(input: &str) -> Option<&'static str> {
    let normalized = input
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let canonical = match normalized.as_str() {
        "codecs" | "codacs" | "code ex" => "codex",
        "open code" => "opencode",
        "cursor agent" => "agent",
        "terminal" | "shell" => LOGIN_SHELL_SENTINEL,
        other => other,
    };
    AGENT_PRESETS
        .iter()
        .find(|preset| preset.command.eq_ignore_ascii_case(canonical))
        .map(|preset| preset.command)
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

pub(crate) fn workspace_item_area(
    sidebar_area: Rect,
    workspace_names: &[String],
    index: usize,
    active_workspace_index: usize,
) -> Rect {
    let x = workspace_item_x(sidebar_area, workspace_names, index, active_workspace_index);
    let desired = if index < workspace_names.len() {
        let show_menu = workspace_tab_shows_menu(index, active_workspace_index);
        workspace_tab_width(&workspace_names[index], show_menu)
    } else {
        WORKSPACE_TAB_ADD_BUTTON_WIDTH
    };
    let max_width = sidebar_area.right().saturating_sub(x);
    Rect {
        x,
        y: sidebar_area.y.saturating_add(WORKSPACE_ENTRY_MARGIN_TOP),
        width: desired.min(max_width),
        height: WORKSPACE_ENTRY_HEIGHT.min(
            sidebar_area
                .height
                .saturating_sub(WORKSPACE_ENTRY_MARGIN_TOP),
        ),
    }
}

pub(crate) fn workspace_add_button_area(
    sidebar_area: Rect,
    workspace_names: &[String],
    active_workspace_index: usize,
) -> Rect {
    let mut area = workspace_item_area(
        sidebar_area,
        workspace_names,
        workspace_names.len(),
        active_workspace_index,
    );
    area.width = WORKSPACE_TAB_ADD_BUTTON_WIDTH.min(area.width);
    area.y = area.y.saturating_add(area.height.saturating_sub(1));
    area.height = 1.min(area.height);
    area
}

pub(crate) fn workspace_menu_button_area(
    sidebar_area: Rect,
    workspace_names: &[String],
    index: usize,
    active_workspace_index: usize,
) -> Rect {
    let item = workspace_item_area(sidebar_area, workspace_names, index, active_workspace_index);
    let menu_offset = WORKSPACE_TAB_SIDE_PADDING.saturating_add(WORKSPACE_TAB_MENU_COLUMNS);
    Rect {
        x: item.right().saturating_sub(menu_offset),
        y: item.y,
        width: WORKSPACE_TAB_MENU_COLUMNS.min(item.width),
        height: 1.min(item.height),
    }
}

pub(crate) fn workspace_tab_hit_area(
    sidebar_area: Rect,
    workspace_names: &[String],
    index: usize,
    active_workspace_index: usize,
) -> Rect {
    let mut area = workspace_item_area(sidebar_area, workspace_names, index, active_workspace_index);
    area.height = sidebar_area.height.min(2).max(area.height);
    if index > 0 {
        area.x = area.x.saturating_sub(1);
        area.width = area.width.saturating_add(1);
    }
    area.width = area
        .width
        .min(sidebar_area.right().saturating_sub(area.x));
    area
}

pub(crate) fn workspace_hit_index(
    sidebar_area: Rect,
    workspace_names: &[String],
    active_workspace_index: usize,
    x: u16,
    y: u16,
) -> Option<usize> {
    (0..workspace_names.len()).rposition(|idx| {
        contains(
            workspace_tab_hit_area(
                sidebar_area,
                workspace_names,
                idx,
                active_workspace_index,
            ),
            x,
            y,
        )
    })
}

pub(crate) fn workspace_menu_hit_index(
    sidebar_area: Rect,
    workspace_names: &[String],
    active_workspace_index: usize,
    x: u16,
    y: u16,
) -> Option<usize> {
    if active_workspace_index >= workspace_names.len() {
        return None;
    }
    contains(
        workspace_menu_button_area(
            sidebar_area,
            workspace_names,
            active_workspace_index,
            active_workspace_index,
        ),
        x,
        y,
    )
    .then_some(active_workspace_index)
}

pub(crate) fn workspace_add_button_hit(
    sidebar_area: Rect,
    workspace_names: &[String],
    active_workspace_index: usize,
    x: u16,
    y: u16,
) -> bool {
    contains(
        workspace_add_button_area(sidebar_area, workspace_names, active_workspace_index),
        x,
        y,
    )
}

pub(crate) fn render_workspace_sidebar(
    f: &mut ratatui::Frame<'_>,
    sidebar_area: Rect,
    theme: Theme,
    workspace_names: &[String],
    _workspace_summaries: &[String],
    active_workspace_index: usize,
    _commander_selected: bool,
    sidebar_workspace_selected_index: Option<usize>,
    sidebar_add_button_selected: bool,
) {
    if sidebar_area.width == 0 || sidebar_area.height == 0 {
        return;
    }

    let divider_x = sidebar_area
        .x
        .saturating_add(commander_panel_width(sidebar_area))
        .min(sidebar_area.right().saturating_sub(1));

    f.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        sidebar_area,
    );

    for (idx, name) in workspace_names.iter().enumerate() {
        let item_area =
            workspace_item_area(sidebar_area, workspace_names, idx, active_workspace_index);
        if item_area.width == 0 || item_area.height == 0 {
            continue;
        }
        let active = idx == active_workspace_index;
        let keyboard_selected = sidebar_workspace_selected_index == Some(idx);
        let style = if active {
            selected_tab_style(theme)
        } else {
            inactive_workspace_tab_style(theme, keyboard_selected)
        };
        if active {
            f.render_widget(Block::default().style(style), item_area);
        }
        let menu_area =
            workspace_menu_button_area(sidebar_area, workspace_names, idx, active_workspace_index);
        let title_columns = workspace_tab_title_columns(item_area.width, active);
        let label = truncate_to_width(name, title_columns as usize);
        f.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Left)
                .style(style),
            Rect {
                x: item_area.x.saturating_add(WORKSPACE_TAB_SIDE_PADDING),
                y: item_area.y,
                width: title_columns,
                height: 1,
            },
        );
        if active && menu_area.width > 0 && menu_area.height > 0 {
            f.render_widget(
                Paragraph::new("⋮")
                    .alignment(Alignment::Center)
                    .style(style),
                menu_area,
            );
        }
    }

    render_workspace_tab_chrome(
        f,
        sidebar_area,
        theme,
        workspace_names,
        active_workspace_index,
        divider_x,
    );

    let add_area = workspace_add_button_area(sidebar_area, workspace_names, active_workspace_index);
    if add_area.width > 0 && add_area.height > 0 {
        let style = if sidebar_add_button_selected {
            selected_tab_style(theme)
        } else {
            inactive_workspace_tab_style(theme, false)
        };
        if sidebar_add_button_selected {
            f.render_widget(Block::default().style(style), add_area);
        }
        f.render_widget(
            Paragraph::new(" +")
                .alignment(Alignment::Left)
                .style(style),
            Rect {
                x: add_area.x,
                y: add_area.y,
                width: add_area.width,
                height: 1,
            },
        );
    }

    let full_size = f.size();
    let line_height = full_size.bottom().saturating_sub(sidebar_area.bottom());
    if line_height > 0 && divider_x < full_size.right() {
        let line = Text::from(vec![Line::from("│"); line_height as usize]);
        f.render_widget(
            Paragraph::new(line)
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            Rect {
                x: divider_x,
                y: sidebar_area.bottom(),
                width: 1,
                height: line_height,
            },
        );
    }
}

pub(crate) fn render_top_chrome(
    f: &mut ratatui::Frame<'_>,
    size: Rect,
    theme: Theme,
    usage_summary: Option<&str>,
) {
    if size.width == 0 || size.height < 2 {
        return;
    }

    let title_row = Rect {
        x: size.x,
        y: size.y,
        width: size.width,
        height: 1,
    };
    if title_row.y >= size.bottom() {
        return;
    }

    let handle_width = APP_HANDLE.chars().count() as u16;
    let handle_area = Rect {
        x: title_row
            .x
            .saturating_add(title_row.width.saturating_sub(handle_width)),
        y: title_row.y,
        width: handle_width.min(title_row.width),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(APP_HANDLE)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        handle_area,
    );

    let left_width = title_row
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
            x: title_row
                .x
                .saturating_add(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
            y: title_row.y,
            width: left_width.saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
            height: 1,
        },
    );

    let Some(usage_summary) = usage_summary else {
        return;
    };
    let usage_summary = usage_summary.trim();
    if usage_summary.is_empty() {
        return;
    }
    let stats_row = Rect {
        x: size.x,
        y: size.y.saturating_add(1),
        width: size.width,
        height: 1,
    };
    if stats_row.y >= size.bottom() || stats_row.width == 0 {
        return;
    }
    let stats_width = stats_row
        .width
        .saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_add(2)) as usize;
    if stats_width == 0 {
        return;
    }
    let stats_text = truncate_to_width(usage_summary, stats_width);
    f.render_widget(
        Paragraph::new(stats_text.to_string())
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        Rect {
            x: stats_row
                .x
                .saturating_add(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
            y: stats_row.y,
            width: stats_row
                .width
                .saturating_sub(WORKSPACE_ENTRY_MARGIN_X.saturating_add(1)),
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

pub(crate) fn panel_settings_modal_area(pane_area: Rect, anchor_title: &str) -> Rect {
    let width = 40.min(pane_area.width);
    let height = 16.min(pane_area.height);
    pane_combobox_dropdown_area(pane_area, anchor_title, width, height)
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

pub(crate) fn new_pane_picker_modal_area(pane_area: Rect, anchor_title: &str) -> Rect {
    let width = 26.min(pane_area.width);
    let height = (AGENT_PRESETS.len() as u16 + 9).min(pane_area.height);
    pane_combobox_dropdown_area(pane_area, anchor_title, width, height)
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
    pane_area: Rect,
    anchor_title: &str,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    cursor: usize,
    name_selected: bool,
    agent_index: usize,
    agent_available: &[bool],
) {
    let area = new_pane_picker_modal_area(pane_area, anchor_title);
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
    pane_area: Rect,
    anchor_title: &str,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    agent_index: usize,
    focus: PanelSettingsFocus,
    agent_available: &[bool],
) {
    let area = panel_settings_modal_area(pane_area, anchor_title);
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
