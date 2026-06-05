use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

/// UI surfaces always use Reset so the terminal background shows through.
pub(crate) fn bg_color(_theme: Theme) -> Color {
    Color::Reset
}

use crate::{
    git_status::GitSummary,
    git_worktree::{relative_subpath_from_git_root, WorktreeInfo},
    layout::{
        clip_rect_to_frame, pane_combobox_dropdown_area, pane_dropdown_max_height,
        pane_subtitle_combobox_dropdown_area, pane_title_bar_height, pane_title_line_layout,
        PANE_TITLE_BAR_HEIGHT,
    },
    theme::Theme,
    theme::THEMES,
    utils::{contains, LOGIN_SHELL_SENTINEL},
};

pub(crate) fn pane_chrome_title_label(pane_title: &str, command: &str) -> String {
    format!("{pane_title} {{{}}}", agent_label_for_command(command))
}

pub(crate) fn commander_chrome_title_label() -> String {
    "Commander {harness}".to_string()
}

fn pane_title_body_spans(
    title: &str,
    title_row_style: Style,
    title_row_selected: bool,
    theme: Theme,
) -> Vec<Span<'static>> {
    let accent_style = Style::default().fg(theme.accent).bg(bg_color(theme));
    let muted_style = Style::default().fg(theme.muted).bg(bg_color(theme));
    let brace_style = if title_row_selected {
        accent_style
    } else {
        muted_style
    };

    if let Some(open_brace) = title.rfind(" {") {
        let type_start = open_brace + 2;
        if title.ends_with('}') && type_start < title.len() {
            let type_end = title.len() - 1;
            return vec![
                Span::styled(title[..open_brace].to_string(), title_row_style),
                Span::styled(" ".to_string(), title_row_style),
                Span::styled("{".to_string(), brace_style),
                Span::styled(title[type_start..type_end].to_string(), muted_style),
                Span::styled("}".to_string(), brace_style),
            ];
        }
    }

    vec![Span::styled(title.to_string(), title_row_style)]
}

fn pane_bottom_line(bottom_width: usize) -> String {
    match bottom_width {
        0 => String::new(),
        1 => "╰".to_string(),
        2 => "╰╯".to_string(),
        width => {
            let mut line = String::with_capacity(width);
            line.push('╰');
            line.extend(std::iter::repeat_n('─', width - 2));
            line.push('╯');
            line
        }
    }
}

fn render_screen_edges(
    f: &mut ratatui::Frame<'_>,
    panel_area: Rect,
    title_y: u16,
    edge_style: Style,
    touches_outer_left: bool,
    touches_outer_right: bool,
    touches_outer_bottom: bool,
) {
    if panel_area.width == 0 || panel_area.height == 0 {
        return;
    }

    let bottom_y = panel_area.bottom().saturating_sub(1);
    // Start below the top title row (`╭─` occupies row 0); Commander's second
    // title row still needs the screen-edge verticals.
    let vertical_top = title_y.saturating_add(1);
    let vertical_bottom = if touches_outer_bottom {
        bottom_y.saturating_sub(1)
    } else {
        bottom_y
    };

    if vertical_top <= vertical_bottom {
        let vertical_height = vertical_bottom.saturating_sub(vertical_top) + 1;
        if touches_outer_left {
            let column = "│\n".repeat(vertical_height.saturating_sub(1) as usize) + "│";
            f.render_widget(
                Paragraph::new(column)
                    .alignment(Alignment::Left)
                    .style(edge_style),
                Rect {
                    x: panel_area.x,
                    y: vertical_top,
                    width: 1,
                    height: vertical_height,
                },
            );
        }
        if touches_outer_right {
            let column = "│\n".repeat(vertical_height.saturating_sub(1) as usize) + "│";
            f.render_widget(
                Paragraph::new(column)
                    .alignment(Alignment::Left)
                    .style(edge_style),
                Rect {
                    x: panel_area.right().saturating_sub(1),
                    y: vertical_top,
                    width: 1,
                    height: vertical_height,
                },
            );
        }
    }

    if touches_outer_bottom && panel_area.width >= 1 {
        f.render_widget(
            Paragraph::new(pane_bottom_line(panel_area.width as usize))
                .alignment(Alignment::Left)
                .style(edge_style),
            Rect {
                x: panel_area.x,
                y: bottom_y,
                width: panel_area.width,
                height: 1,
            },
        );
    }
}

/// Title bar chrome shared by workspace panes (single row) and Commander (two rows).
pub(crate) fn render_panel_title_chrome(
    f: &mut ratatui::Frame<'_>,
    panel_area: Rect,
    title: &str,
    folder_name: Option<&str>,
    git_summary: Option<&GitSummary>,
    subpath: Option<&str>,
    is_commander: bool,
    title_row_style: Style,
    title_row_selected: bool,
    edge_style: Style,
    theme: Theme,
    show_window_controls: bool,
    is_maximized: bool,
    show_right_edge: bool,
    show_bottom_edge: bool,
    show_left_edge: bool,
) {
    use crate::layout::{
        pane_title_bar_area, pane_title_chrome_reserve, pane_title_controls_icons_area,
        pane_title_top_right_corner_area, pane_title_y, PANE_CONTROLS_PADDING,
        PANE_TITLE_LEFT_PADDING, PANE_TITLE_TEXT_PADDING,
    };

    let title_bar_height = pane_title_bar_height(is_commander);
    let title_bar = pane_title_bar_area(panel_area, title_bar_height);
    let title_y = pane_title_y(panel_area);
    let title_bar_width = title_bar.width;
    let inline_subtitle = !is_commander;

    if title_y < panel_area.bottom() && title_bar_width > PANE_TITLE_LEFT_PADDING {
        let chrome_reserve = if show_window_controls {
            pane_title_chrome_reserve(panel_area.width, show_right_edge)
        } else {
            0
        };
        let text_x = title_bar.x.saturating_add(PANE_TITLE_LEFT_PADDING);
        let text_width = title_bar_width
            .saturating_sub(PANE_TITLE_LEFT_PADDING)
            .saturating_sub(chrome_reserve);

        if text_width > PANE_TITLE_TEXT_PADDING.saturating_mul(2) {
            let rule_glyph = if title_row_selected { '═' } else { '─' };

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("╭─ ", title_row_style),
                    Span::styled(
                        PANE_TITLE_CORNER_ICON.to_string(),
                        Style::default().fg(Color::White).bg(bg_color(theme)),
                    ),
                ]))
                .alignment(Alignment::Left),
                Rect {
                    x: title_bar.x,
                    y: title_y,
                    width: PANE_TITLE_LEFT_PADDING,
                    height: 1,
                },
            );

            let title_max = text_width
                .saturating_sub(PANE_TITLE_TEXT_PADDING.saturating_mul(2)) as usize;
            let title_layout = pane_title_line_layout(
                title,
                folder_name,
                git_summary,
                subpath,
                title_max,
                inline_subtitle,
            );
            const TITLE_RULE_GAP: u16 = 1;
            let title_content_width = if inline_subtitle {
                title_layout.total_width()
            } else {
                1 + truncate_to_width(title, title_max).chars().count()
            };
            let title_rule_start =
                text_x.saturating_add(title_content_width as u16 + TITLE_RULE_GAP);

            if inline_subtitle {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(pane_title_body_spans(
                    &title_layout.title_body,
                    title_row_style,
                    title_row_selected,
                    theme,
                ));
                if let Some((icon, name, branch_tail, git_color, subpath_tail)) =
                    title_layout.subtitle
                {
                    let icon_style =
                        Style::default().fg(theme.foreground).bg(bg_color(theme));
                    let name_style = Style::default()
                        .fg(selected_tab_bg(theme))
                        .bg(bg_color(theme));
                    let muted_style = Style::default().fg(theme.muted).bg(bg_color(theme));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(icon.to_string(), icon_style));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(name, name_style));
                    if !branch_tail.is_empty() {
                        let branch_style = git_color
                            .map(|color| Style::default().fg(color).bg(bg_color(theme)))
                            .unwrap_or(muted_style);
                        spans.push(Span::styled(branch_tail, branch_style));
                    }
                    if !subpath_tail.is_empty() {
                        spans.push(Span::styled(subpath_tail, muted_style));
                    }
                }
                f.render_widget(
                    Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
                    Rect {
                        x: text_x,
                        y: title_y,
                        width: text_width,
                        height: 1,
                    },
                );
            } else {
                let title_body = truncate_to_width(title, title_max);
                let mut spans = vec![Span::raw(" ")];
                spans.extend(pane_title_body_spans(
                    &title_body,
                    title_row_style,
                    title_row_selected,
                    theme,
                ));
                f.render_widget(
                    Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
                    Rect {
                        x: text_x,
                        y: title_y,
                        width: text_width,
                        height: 1,
                    },
                );
            }

            if show_window_controls {
                if let Some(icons_area) =
                    pane_title_controls_icons_area(panel_area, show_right_edge)
                {
                    let maximize_icon = if is_maximized { "🗗" } else { "⛶" };
                    let pad = " ".repeat(PANE_CONTROLS_PADDING as usize);
                    let controls_text =
                        format!("{pad}↻{pad}{maximize_icon}{pad}✕{pad}");

                    let bar_start = title_rule_start;
                    let bar_width = icons_area.x.saturating_sub(bar_start);
                    if bar_width > 0 {
                        f.render_widget(
                            Paragraph::new(rule_glyph.to_string().repeat(bar_width as usize))
                                .alignment(Alignment::Left)
                                .style(title_row_style),
                            Rect {
                                x: bar_start,
                                y: title_y,
                                width: bar_width,
                                height: 1,
                            },
                        );
                    }
                    f.render_widget(
                        Paragraph::new(controls_text).style(title_row_style),
                        Rect {
                            x: icons_area.x,
                            y: icons_area.y,
                            width: icons_area.width,
                            height: 1,
                        },
                    );
                    if let Some(corner_area) =
                        pane_title_top_right_corner_area(panel_area, show_right_edge)
                    {
                        f.render_widget(
                            Paragraph::new("─╮")
                                .alignment(Alignment::Left)
                                .style(title_row_style),
                            corner_area,
                        );
                    }
                }
            }
        }
    }

    if panel_area.width > 0 && panel_area.height > 0 {
        let touches_outer_bottom = !show_bottom_edge;
        let touches_outer_left = !show_left_edge;
        let touches_outer_right = !show_right_edge;

        if touches_outer_left || touches_outer_right || touches_outer_bottom {
            render_screen_edges(
                f,
                panel_area,
                title_y,
                edge_style,
                touches_outer_left,
                touches_outer_right,
                touches_outer_bottom,
            );
        }

        if show_bottom_edge && panel_area.width >= 2 {
            let bottom_y = panel_area.bottom().saturating_sub(1);
            let bottom_line = pane_bottom_line(panel_area.width as usize);
            f.render_widget(
                Paragraph::new(bottom_line)
                    .alignment(Alignment::Left)
                    .style(edge_style),
                Rect {
                    x: panel_area.x,
                    y: bottom_y,
                    width: panel_area.width,
                    height: 1,
                },
            );
        } else if show_right_edge && panel_area.width < 2 {
            let bottom_y = panel_area.bottom().saturating_sub(1);
            f.render_widget(
                Paragraph::new("│")
                    .alignment(Alignment::Left)
                    .style(edge_style),
                Rect {
                    x: panel_area.right().saturating_sub(1),
                    y: bottom_y,
                    width: 1,
                    height: 1,
                },
            );
        }
    }
}

pub(crate) const COMMANDER_COMMAND: &str = "commander";
const PANE_TITLE_CORNER_ICON: char = '\u{F04FD}';
pub(crate) const TOP_CHROME_ROWS: u16 = 1;
const RIGHT_CLUSTER_SEP: &str = " │ ";
const APP_NAME: &str = "Code UI";
const APP_VERSION: &str = "0.0.2";
const APP_HANDLE: &str = "@motionharvest";
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct TopBarLayout {
    pub bar: Rect,
    pub right_cluster: Rect,
}

pub(crate) fn top_bar_area(size: Rect) -> Rect {
    Rect {
        x: size.x,
        y: size.y,
        width: size.width,
        height: TOP_CHROME_ROWS.min(size.height),
    }
}

fn right_cluster_width(usage_summary: Option<&str>) -> u16 {
    let handle_width = APP_HANDLE.chars().count() as u16;
    let app_label_width = APP_NAME
        .chars()
        .count()
        .saturating_add(1)
        .saturating_add(APP_VERSION.chars().count()) as u16;
    let usage_summary = usage_summary
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let usage_full_width = usage_summary
        .map(|text| text.chars().count() as u16)
        .unwrap_or(0);
    let sep_width = RIGHT_CLUSTER_SEP.chars().count() as u16;
    let mut width = app_label_width;
    if usage_full_width > 0 {
        width = width
            .saturating_add(sep_width)
            .saturating_add(usage_full_width);
    }
    width.saturating_add(sep_width).saturating_add(handle_width)
}

pub(crate) fn compute_top_bar_layout(
    size: Rect,
    _workspace_names: &[String],
    _active_workspace_index: usize,
    usage_summary: Option<&str>,
) -> TopBarLayout {
    let bar = top_bar_area(size);
    let cluster_width = right_cluster_width(usage_summary).min(bar.width);
    let right_cluster = Rect {
        x: bar
            .x
            .saturating_add(bar.width.saturating_sub(cluster_width)),
        y: bar.y,
        width: cluster_width,
        height: 1,
    };

    TopBarLayout {
        bar,
        right_cluster,
    }
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

fn theme_surface_color(theme: Theme) -> Color {
    theme
        .palette
        .first()
        .copied()
        .filter(|color| *color != Color::Reset)
        .unwrap_or(theme.foreground)
}

fn selected_tab_bg(theme: Theme) -> Color {
    let candidate = theme.palette.get(14).copied().unwrap_or(theme.accent);
    match color_contrast_delta(candidate, theme_surface_color(theme)) {
        Some(delta) if delta >= 120 => candidate,
        _ => theme.accent,
    }
}

fn selected_tab_fg(theme: Theme, tab_bg: Color) -> Color {
    let surface = theme.palette.first().copied().unwrap_or(Color::Reset);
    if surface != Color::Reset {
        return surface;
    }
    match color_to_rgb(tab_bg) {
        Some((r, g, b)) if u16::from(r) + u16::from(g) + u16::from(b) > 382 => Color::Black,
        _ => Color::White,
    }
}

fn selected_tab_style(theme: Theme) -> Style {
    let bg = selected_tab_bg(theme);
    Style::default()
        .fg(selected_tab_fg(theme, bg))
        .bg(bg)
        .add_modifier(Modifier::BOLD)
}

fn inactive_workspace_tab_style(theme: Theme, keyboard_focused: bool) -> Style {
    let mut style = Style::default().fg(theme.muted).bg(bg_color(theme));
    if keyboard_focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn workspace_tabs_start_x(sidebar_area: Rect) -> u16 {
    sidebar_area.x
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

fn render_workspace_tab_chrome(
    f: &mut ratatui::Frame<'_>,
    sidebar_area: Rect,
    theme: Theme,
    workspace_names: &[String],
    active_workspace_index: usize,
) {
    if sidebar_area.height == 0 || workspace_names.is_empty() {
        return;
    }

    let tab_row_style = Style::default().fg(theme.muted).bg(bg_color(theme));

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
}

/// Truncate to a maximum number of terminal cells (counted as Unicode scalar
/// values, matching ratatui's default monospace assumption).
fn place_row(inner: Rect, y: u16, height: u16) -> (Rect, u16) {
    if height == 0 || y >= inner.bottom() {
        return (
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 0,
            },
            y,
        );
    }
    let row_height = height.min(inner.bottom().saturating_sub(y));
    (
        Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: row_height,
        },
        y.saturating_add(row_height),
    )
}

/// Render list lines one row at a time so content cannot paint past `area`.
pub(crate) fn render_lines_in_area(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    lines: Vec<Line<'static>>,
    style: Style,
    frame: Rect,
) {
    let area = clip_rect_to_frame(area, frame);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_rows = area.height as usize;
    for (row, line) in lines.into_iter().enumerate().take(max_rows) {
        let row_area = clip_rect_to_frame(
            Rect {
                x: area.x,
                y: area.y.saturating_add(row as u16),
                width: area.width,
                height: 1,
            },
            frame,
        );
        if row_area.height == 0 {
            break;
        }
        f.render_widget(Paragraph::new(line).style(style), row_area);
    }
}

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

pub(crate) const AGENT_PRESETS: [AgentPreset; 6] = [
    AgentPreset {
        label: "Terminal",
        command: LOGIN_SHELL_SENTINEL,
        binary: None,
    },
    AgentPreset {
        label: "Commander",
        command: COMMANDER_COMMAND,
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
    WorktreePicker {
        pane_id: usize,
        folder_name: String,
        git_summary: GitSummary,
        repo_root: std::path::PathBuf,
        target_branch: String,
        entries: Vec<WorktreeInfo>,
        entry_summaries: Vec<Option<GitSummary>>,
        entry_snapshots: Vec<crate::git_worktree::WorktreeGitSnapshot>,
        entry_states: Vec<crate::worktree_lifecycle::WorktreeLifecycleState>,
        current_path: Option<std::path::PathBuf>,
        selected_index: usize,
        list_column: crate::worktree_ui::WorktreeListColumn,
        focus: crate::worktree_ui::WorktreePickerFocus,
        submodal: crate::worktree_ui::WorktreeSubmodal,
        delete_target_index: Option<usize>,
        delete_action_index: usize,
        runtime_flags: Vec<(std::path::PathBuf, crate::worktree_lifecycle::WorktreeRuntimeFlag)>,
        check_results: std::collections::HashMap<usize, crate::git_status::CheckResults>,
        error_message: Option<String>,
        cursor: usize,
    },
}

pub(crate) use crate::worktree_ui::WorktreePickerFocus;

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
    area.height = sidebar_area.height.min(1).max(area.height);
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
    layout: TopBarLayout,
    theme: Theme,
    workspace_names: &[String],
    _workspace_summaries: &[String],
    active_workspace_index: usize,
    sidebar_workspace_selected_index: Option<usize>,
    sidebar_add_button_selected: bool,
) {
    let sidebar_area = layout.bar;
    if sidebar_area.width == 0 || sidebar_area.height == 0 {
        return;
    }

    f.render_widget(
        Block::default().style(Style::default().bg(bg_color(theme))),
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
}

pub(crate) fn render_top_chrome(
    f: &mut ratatui::Frame<'_>,
    layout: TopBarLayout,
    theme: Theme,
    usage_summary: Option<&str>,
) {
    let cluster = layout.right_cluster;
    if cluster.width == 0 || cluster.height == 0 {
        return;
    }

    let usage_text = usage_summary.map(str::trim).filter(|t| !t.is_empty());
    let mut spans: Vec<Span<'_>> = vec![
        Span::styled(
            APP_NAME,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(APP_VERSION, Style::default().fg(theme.muted)),
    ];
    if let Some(usage) = usage_text {
        spans.push(Span::styled(
            RIGHT_CLUSTER_SEP,
            Style::default().fg(theme.muted).bg(bg_color(theme)),
        ));
        spans.push(Span::styled(
            usage,
            Style::default().fg(theme.muted).bg(bg_color(theme)),
        ));
    }
    spans.push(Span::styled(
        RIGHT_CLUSTER_SEP,
        Style::default().fg(theme.muted).bg(bg_color(theme)),
    ));
    spans.push(Span::styled(
        APP_HANDLE,
        Style::default().fg(theme.muted).bg(bg_color(theme)),
    ));
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Left)
            .style(Style::default().bg(bg_color(theme))),
        cluster,
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let close_area = help_close_button_area(area);
    let close_button = Paragraph::new("✕")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
    f.render_widget(close_button, close_area);

    let debug_area = help_debug_toggle_button_area(area);
    let debug_button = Paragraph::new(if debug_containers {
        "[Containers: ON]"
    } else {
        "[Containers: OFF]"
    })
    .alignment(Alignment::Left)
    .style(if debug_containers {
        Style::default().fg(theme.accent).bg(bg_color(theme))
    } else {
        Style::default().fg(theme.muted).bg(bg_color(theme))
    });
    f.render_widget(debug_button, debug_area);

    let theme_line = Paragraph::new(format!("Current theme: {}", theme.name))
        .style(Style::default().fg(theme.muted).bg(bg_color(theme)));
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
Ctrl+Q twice: Quit\n\
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
Wheel / PageUp/PageDown: Scroll pane history (3 lines per wheel notch)\n\
Shift+PageUp/PageDown: Scroll by page\n\
Shift+Home/End: Scroll to top/bottom\n\
Esc: Return to live output when scrolled up",
    )
    .style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .wrap(Wrap { trim: false });
    f.render_widget(list, inner);
}

pub(crate) fn panel_settings_modal_area(
    pane_area: Rect,
    anchor_title: &str,
    frame: Rect,
    title_bar_height: u16,
) -> Rect {
    let width = 40.min(pane_area.width);
    let height = 16.min(pane_dropdown_max_height(pane_area, title_bar_height));
    pane_combobox_dropdown_area(
        pane_area,
        frame,
        anchor_title,
        width,
        height,
        title_bar_height,
    )
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

const NEW_PANE_TITLE_PREFIX: &str = "─ ";
const NEW_PANE_TITLE_SUFFIX: &str = " ─";
/// Rows below the pane's top edge where the picker modal begins.
const NEW_PANE_PICKER_Y_INSET_FROM_PANE_TOP: u16 = 0;
/// Columns to shift the picker left from the title-aligned anchor.
const NEW_PANE_PICKER_X_SHIFT_LEFT: u16 = 2;

pub(crate) struct NewPanePickerLayout {
    pub title_row: Rect,
    pub list: Rect,
}

fn new_pane_picker_title_area(modal_area: Rect) -> Rect {
    Rect {
        x: modal_area.x.saturating_add(1),
        y: modal_area.y,
        width: modal_area.width.saturating_sub(2),
        height: 1,
    }
}

fn new_pane_picker_title_prefix_cols() -> usize {
    NEW_PANE_TITLE_PREFIX.chars().count()
}

pub(crate) fn new_pane_picker_title_name_prefix_cols() -> usize {
    new_pane_picker_title_prefix_cols()
}

fn new_pane_picker_title_suffix_cols() -> usize {
    NEW_PANE_TITLE_SUFFIX.chars().count()
}

fn new_pane_picker_title_name_capacity(title_width: u16) -> usize {
    title_width
        .saturating_sub(
            (new_pane_picker_title_prefix_cols() + new_pane_picker_title_suffix_cols()) as u16,
        )
        .max(1) as usize
}

pub(crate) fn new_pane_picker_name_cursor_position(
    title_row: Rect,
    name: &str,
    cursor: usize,
    name_selected: bool,
) -> (u16, u16) {
    let prefix = new_pane_picker_title_prefix_cols();
    let cursor_col = if name_selected {
        name.chars().count()
    } else {
        cursor.min(name.chars().count())
    };
    let x = title_row
        .x
        .saturating_add((prefix + cursor_col) as u16)
        .min(title_row.right().saturating_sub(1));
    (x, title_row.y)
}

pub(crate) fn new_pane_picker_modal_area(
    pane_area: Rect,
    anchor_title: &str,
    frame: Rect,
    title_bar_height: u16,
) -> Rect {
    let width = 26.min(pane_area.width);
    let desired = AGENT_PRESETS.len() as u16 + 2;
    let height = desired
        .min(pane_dropdown_max_height(pane_area, title_bar_height))
        .max(1);
    let mut area = pane_combobox_dropdown_area(
        pane_area,
        frame,
        anchor_title,
        width,
        height,
        title_bar_height,
    );
    area.y = pane_area
        .y
        .saturating_add(NEW_PANE_PICKER_Y_INSET_FROM_PANE_TOP)
        .min(pane_area.bottom().saturating_sub(height));
    area.x = area
        .x
        .saturating_sub(NEW_PANE_PICKER_X_SHIFT_LEFT)
        .max(pane_area.x);
    clip_rect_to_frame(area, frame)
}

pub(crate) fn new_pane_picker_layout(modal_area: Rect) -> NewPanePickerLayout {
    let title_row = new_pane_picker_title_area(modal_area);
    let inner = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .inner(modal_area);
    let (list, _) = place_row(inner, inner.y, inner.height);
    NewPanePickerLayout { title_row, list }
}

pub(crate) fn render_new_pane_picker_modal(
    f: &mut ratatui::Frame<'_>,
    pane_area: Rect,
    anchor_title: &str,
    title_bar_height: u16,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    cursor: usize,
    name_selected: bool,
    agent_index: usize,
    agent_available: &[bool],
) {
    let frame = f.size();
    let area = new_pane_picker_modal_area(pane_area, anchor_title, frame, title_bar_height);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(Style::default().fg(theme.accent));
    f.render_widget(block, area);

    let layout = new_pane_picker_layout(area);
    let list_style = Style::default().fg(theme.foreground).bg(bg_color(theme));

    let title_row = clip_rect_to_frame(layout.title_row, frame);
    if title_row.height > 0 && title_row.width > 0 {
        let name_capacity = new_pane_picker_title_name_capacity(title_row.width);
        let visible_name = truncate_to_width(name, name_capacity);
        let chrome_style = Style::default().fg(theme.muted).bg(bg_color(theme));
        let mut name_style = if name_selected {
            Style::default().fg(bg_color(theme)).bg(theme.accent)
        } else if name_error.is_some() {
            Style::default().fg(Color::Red).bg(bg_color(theme))
        } else {
            Style::default().fg(theme.foreground).bg(bg_color(theme))
        };
        if name_error.is_some() && !name_selected {
            name_style = name_style.add_modifier(Modifier::UNDERLINED);
        }
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(NEW_PANE_TITLE_PREFIX, chrome_style),
                Span::styled(visible_name, name_style),
                Span::styled(NEW_PANE_TITLE_SUFFIX, chrome_style),
            ]))
            .style(Style::default().bg(bg_color(theme))),
            title_row,
        );
    }

    let list_area = clip_rect_to_frame(layout.list, frame);
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

    render_lines_in_area(f, list_area, lines, list_style, frame);

    if title_row.height > 0 {
        let (cursor_x, cursor_y) =
            new_pane_picker_name_cursor_position(title_row, name, cursor, name_selected);
        f.set_cursor(cursor_x, cursor_y);
    }
}

pub(crate) fn render_panel_settings_modal(
    f: &mut ratatui::Frame<'_>,
    pane_area: Rect,
    anchor_title: &str,
    title_bar_height: u16,
    theme: Theme,
    name: &str,
    name_error: Option<&str>,
    agent_index: usize,
    focus: PanelSettingsFocus,
    agent_available: &[bool],
) {
    let frame = f.size();
    let area = panel_settings_modal_area(pane_area, anchor_title, frame, title_bar_height);
    f.render_widget(Clear, area);

    let inner = panel_settings_modal_inner(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Panel Settings")
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(Style::default().fg(theme.accent));
    f.render_widget(block, area);

    let close_area = panel_settings_close_button_area(area);
    let close_button = Paragraph::new("✕")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
    f.render_widget(close_button, close_area);

    let hint = Paragraph::new("Tab: next field   Esc: cancel   Enter: confirm")
        .style(Style::default().fg(theme.muted).bg(bg_color(theme)));
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
        Paragraph::new("Name").style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
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
            .style(Style::default().fg(theme.foreground).bg(bg_color(theme))),
        name_inner,
    );
    if let Some(error) = name_error {
        f.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red).bg(bg_color(theme))),
            Rect {
                x: inner.x,
                y: name_area.bottom(),
                width: inner.width,
                height: 1,
            },
        );
    }

    let agent_label =
        Paragraph::new("Agent").style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
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
            .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
            .wrap(Wrap { trim: false }),
        agent_inner,
    );

    let cancel = Paragraph::new("[Cancel]")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted).bg(bg_color(theme)));
    f.render_widget(cancel, panel_settings_cancel_button_area(area));

    let confirm = Paragraph::new("[Confirm]")
        .alignment(Alignment::Center)
        .style(if focus == PanelSettingsFocus::Confirm {
            Style::default().fg(theme.accent).bg(bg_color(theme))
        } else {
            Style::default().fg(theme.foreground).bg(bg_color(theme))
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let hint = Paragraph::new("Type name, L/R cursor, U/D action")
        .style(Style::default().fg(theme.muted).bg(bg_color(theme)));
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
        Paragraph::new("Name").style(Style::default().fg(theme.foreground).bg(bg_color(theme)));
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
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(if name_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(theme.accent)
        });
    let name_inner = name_block.inner(name_area);
    f.render_widget(name_block, name_area);
    f.render_widget(
        Paragraph::new(name.to_string())
            .style(Style::default().fg(theme.foreground).bg(bg_color(theme))),
        name_inner,
    );

    if let Some(error) = name_error {
        f.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red).bg(bg_color(theme))),
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
            .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
            .wrap(Wrap { trim: false }),
        actions_area,
    );

    let cursor_x = name_inner
        .x
        .saturating_add(cursor.min(name_inner.width.saturating_sub(1) as usize) as u16);
    f.set_cursor(cursor_x, name_inner.y);
}

pub(crate) use crate::worktree_ui::worktree_picker_item_count;

pub(crate) fn worktree_picker_modal_width(
    entries: &[WorktreeInfo],
    entry_states: &[crate::worktree_lifecycle::WorktreeLifecycleState],
    repo_root: &std::path::Path,
) -> u16 {
    crate::worktree_ui::worktree_picker_modal_width(entries, entry_states, repo_root)
}

pub(crate) fn worktree_picker_modal_height(
    entries: &[WorktreeInfo],
    focus: WorktreePickerFocus,
    submodal: &crate::worktree_ui::WorktreeSubmodal,
) -> u16 {
    crate::worktree_ui::worktree_picker_modal_height(entries, focus, submodal)
}

pub(crate) fn worktree_picker_modal_area(
    pane_area: Rect,
    frame: Rect,
    chrome_title: &str,
    folder_name: &str,
    git_summary: &GitSummary,
    subpath: Option<&str>,
    entries: &[WorktreeInfo],
    entry_states: &[crate::worktree_lifecycle::WorktreeLifecycleState],
    repo_root: &std::path::Path,
    focus: WorktreePickerFocus,
    submodal: &crate::worktree_ui::WorktreeSubmodal,
) -> Rect {
    let width = worktree_picker_modal_width(entries, entry_states, repo_root).min(pane_area.width);
    let height = worktree_picker_modal_height(entries, focus, submodal)
        .min(pane_dropdown_max_height(pane_area, PANE_TITLE_BAR_HEIGHT));
    pane_subtitle_combobox_dropdown_area(
        pane_area,
        frame,
        chrome_title,
        folder_name,
        Some(git_summary),
        subpath,
        width,
        height,
        true,
        PANE_TITLE_BAR_HEIGHT,
    )
}

pub(crate) use crate::worktree_ui::{
    worktree_picker_list_hit_index, worktree_picker_status_column_hit,
};

pub(crate) fn render_worktree_picker_modal(
    f: &mut ratatui::Frame<'_>,
    pane_area: Rect,
    chrome_title: &str,
    folder_name: &str,
    git_summary: &GitSummary,
    theme: Theme,
    entries: &[WorktreeInfo],
    entry_summaries: &[Option<GitSummary>],
    entry_snapshots: &[crate::git_worktree::WorktreeGitSnapshot],
    entry_states: &[crate::worktree_lifecycle::WorktreeLifecycleState],
    repo_root: &std::path::Path,
    target_branch: &str,
    current_path: Option<&std::path::Path>,
    selected_index: usize,
    list_column: crate::worktree_ui::WorktreeListColumn,
    focus: WorktreePickerFocus,
    submodal: &crate::worktree_ui::WorktreeSubmodal,
    delete_target_index: Option<usize>,
    delete_action_index: usize,
) {
    let frame = f.size();
    let subpath = current_path.and_then(relative_subpath_from_git_root);
    let area = worktree_picker_modal_area(
        pane_area,
        frame,
        chrome_title,
        folder_name,
        git_summary,
        subpath.as_deref(),
        entries,
        entry_states,
        repo_root,
        focus,
        submodal,
    );
    crate::worktree_ui::render_worktree_picker(
        f,
        area,
        theme,
        entries,
        entry_states,
        repo_root,
        current_path,
        selected_index,
        list_column,
        focus,
        delete_target_index,
        delete_action_index,
        submodal,
        target_branch,
        entry_snapshots,
        entry_summaries,
    );
}
