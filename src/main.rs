mod app;
mod git_status;
mod git_worktree;
mod layout;
mod pane;
mod theme;
mod ui;
mod utils;
mod worktree_lifecycle;
mod worktree_ui;

use std::{
    env,
    io::{self, Write},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

/// How long we wait for agents to print their resume hint and exit after the
/// user closes the TUI (Ctrl+Q, Ctrl+R, or a layout-collapse close).
const SHUTDOWN_GRACE: Duration = Duration::from_millis(750);
const MOUSE_MOVE_DEBOUNCE: Duration = Duration::from_millis(16);

use anyhow::Context;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame, Terminal,
};

use app::{App, MousePointerShape};
use git_status::GitStatusCache;
use layout::{clip_rect_to_frame, pane_borders, pane_inner_area, pane_title_bar_height};
use theme::Theme;
use ui::{
    commander_chrome_title_label, compute_top_bar_layout,
    render_help_modal, render_new_pane_picker_modal, render_panel_settings_modal,
    render_panel_title_chrome, render_theme_modal, render_top_chrome,
    render_worktree_picker_modal, render_workspace_settings_modal, render_workspace_sidebar,
    bg_color, COMMANDER_COMMAND,
};

fn main() -> anyhow::Result<()> {
    install_terminal_cleanup();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    );
    let _terminal_guard = TerminalGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let size = terminal.size()?;
    let mut app = App::new(size.height, size.width).context("failed to create panes")?;

    let res = run(&mut terminal, &mut app);
    let reload_requested = app.reload_requested;

    // Give each agent a chance to flush its session state and print a resume
    // line before we save and tear down the PTYs. Without this, closing the
    // whole TUI would leave us with whatever resume hints happened to be
    // captured during normal operation (typically none, if the user just hit
    // Ctrl+Q without exiting each agent first).
    app.shutdown_panes(SHUTDOWN_GRACE);

    app.persist_layout();

    restore_terminal(&mut terminal);

    if let Err(err) = res {
        if !(reload_requested && is_input_output_error(&err)) {
            return Err(err);
        }
    }
    if reload_requested {
        restart_app();
    }

    Ok(())
}

static TERMINAL_CLEANUP: OnceLock<()> = OnceLock::new();

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal_state();
    }
}

fn install_terminal_cleanup() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_state();
        default_hook(info);
    }));
}

/// Restore the outer terminal even when the TUI exits abnormally. Without
/// this, mouse tracking stays enabled and the shell prints raw sequences like
/// `35;109;25M` for every mouse move.
fn restore_terminal_state() {
    if TERMINAL_CLEANUP.set(()).is_err() {
        return;
    }

    let mut stdout = io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    let _ = set_mouse_pointer_shape(&mut stdout, MousePointerShape::Default);
    let _ = execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show,
    );
    let _ = stdout.flush();
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    restore_terminal_state();
    let _ = terminal.show_cursor();
}

fn restart_app() {
    if restart_via_current_exe() {
        return;
    }
    if restart_via_script() {
        return;
    }
    eprintln!("failed to restart: could not relaunch current executable or reload.sh");
}

fn restart_via_current_exe() -> bool {
    let Ok(exe) = env::current_exe() else {
        return false;
    };
    let args: Vec<_> = env::args_os().skip(1).collect();
    Command::new(exe).args(args).spawn().is_ok()
}

fn restart_via_script() -> bool {
    Command::new("./reload.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .spawn()
        .is_ok()
}

fn is_input_output_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|io_err| io_err.raw_os_error() == Some(5))
    })
}

fn render_debug_divider(f: &mut ratatui::Frame<'_>, area: Rect, style: Style) {
    let glyphs = if area.width <= 1 {
        vertical_divider_glyphs(area.height)
    } else {
        horizontal_divider_glyphs(area.width)
    };
    f.render_widget(Paragraph::new(glyphs).style(style), area);
}

fn vertical_divider_glyphs(height: u16) -> String {
    match height {
        0 => String::new(),
        1 => "│".to_string(),
        2 => "⊤\n⊥".to_string(),
        _ => {
            let mut out = String::from("⊤");
            for _ in 0..height.saturating_sub(2) {
                out.push('\n');
                out.push('│');
            }
            out.push('\n');
            out.push('⊥');
            out
        }
    }
}

fn horizontal_divider_glyphs(width: u16) -> String {
    match width {
        0 => String::new(),
        1 => "─".to_string(),
        2 => "⊢⊣".to_string(),
        _ => {
            let mut out = String::from("⊢");
            for _ in 0..width.saturating_sub(2) {
                out.push('─');
            }
            out.push('⊣');
            out
        }
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> anyhow::Result<()> {
    let mut cursor_visible = false;
    let mut mouse_capture_enabled = true;
    let mut mouse_pointer_shape = MousePointerShape::Default;
    let mut last_mouse_position: Option<(u16, u16)> = None;
    let mut last_mouse_move_at: Option<std::time::Instant> = None;
    let mut last_size = terminal.size()?;
    app.resize(last_size.height, last_size.width);
    // Force an initial paint.
    let mut dirty = true;
    let mut git_cache = GitStatusCache::new(Duration::from_secs(2));

    while app.running {
        let size = terminal.size()?;
        if size != last_size {
            app.resize(size.height, size.width);
            last_size = size;
            dirty = true;
        }
        if app.tick() {
            dirty = true;
        }

        if dirty {
            app.prepare_commander_palette_video(last_size);
            let mut commander_chat_metrics = None;
            terminal.draw(|f| {
                let theme = if app.modal == Some(ui::Modal::Theme) {
                    app.preview_theme()
                } else {
                    app.theme()
                };
                // Only fill background if theme is not transparent (Reset)
                if theme.background != Color::Reset {
                    f.render_widget(
                        Block::default().style(Style::default().bg(theme.background)),
                        f.size(),
                    );
                }

                let workspace_names = app.workspace_names();
                let workspace_summaries = app.workspace_pane_summaries();
                let top_layout = compute_top_bar_layout(
                    f.size(),
                    &workspace_names,
                    app.active_workspace_index(),
                    None,
                );
                render_workspace_sidebar(
                    f,
                    top_layout,
                    theme,
                    &workspace_names,
                    &workspace_summaries,
                    app.active_workspace_index(),
                    app.sidebar_workspace_focused(),
                    app.sidebar_add_button_focused(),
                );
                render_top_chrome(f, top_layout, theme, None);

                let debug_mode = false;
                let (debug_containers, debug_placements) = if debug_mode {
                    app.debug_layout_areas(App::content_area(f.size()))
                } else {
                    (Vec::new(), Vec::new())
                };

                if debug_mode {
                    for container in &debug_containers {
                        let style = Style::default()
                            .fg(Color::Rgb(148, 0, 211))
                            .add_modifier(Modifier::BOLD);
                        if container.area.width > 0 && container.area.height > 0 {
                            let block = Block::default().borders(Borders::ALL).border_style(style);
                            f.render_widget(block, container.area);
                        }
                        if container.divider_area.width > 0 && container.divider_area.height > 0 {
                            render_debug_divider(f, container.divider_area, style);
                        }
                    }
                }

                let placements = app.pane_placements(App::content_area(f.size()));
                let swap_preview_target = app.pane_swap_preview_target();
                let modal_is_none = app.modal.is_none();
                let focused_pane_id = (app.sidebar_workspace_focused().is_none()
                    && !app.sidebar_add_button_focused())
                .then_some(app.focused);

                for placement in placements {
                    let focused = focused_pane_id == Some(placement.pane_id);
                    let in_swap_preview = swap_preview_target == Some(placement.pane_id);
                    let Some(pane_index) = app
                        .panes
                        .iter()
                        .position(|pane| pane.id == placement.pane_id)
                    else {
                        continue;
                    };
                    let is_maximized = app.is_maximized();
                    let selection = app.pane_selection(placement.pane_id);
                    let pane = &mut app.panes[pane_index];

                    let pane_area = if debug_mode {
                        let Some(debug_placement) = debug_placements
                            .iter()
                            .find(|debug_placement| debug_placement.pane_id == placement.pane_id)
                        else {
                            continue;
                        };

                        let outer = Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(Color::Yellow));
                        f.render_widget(outer, debug_placement.container_area);
                        debug_placement.pane_area
                    } else {
                        placement.area
                    };

                    // Clear the first-paint gate even when the placement rect is
                    // temporarily zero-sized so PTY output is not blocked forever.
                    pane.mark_painted();

                    if pane_area.width == 0 || pane_area.height == 0 {
                        continue;
                    }

                    let title_row_selected = focused || in_swap_preview;
                    let outline_color = if title_row_selected {
                        theme.accent
                    } else {
                        theme.muted
                    };
                    let title_row_style =
                        Style::default().fg(outline_color).bg(bg_color(theme));
                    let edge_style = title_row_style;

                    let block = Block::default()
                        .borders(pane_borders(placement.exposed))
                        .border_type(BorderType::Plain)
                        .style(Style::default().bg(bg_color(theme)))
                        .border_style(edge_style);
                    f.render_widget(block, pane_area);

                    let is_commander = pane.command == COMMANDER_COMMAND;
                    let git_summary = if is_commander {
                        None
                    } else {
                        git_cache.summary_for_pane(pane)
                    };
                    let folder_name = if is_commander {
                        None
                    } else {
                        git_cache.folder_name_for_pane(pane)
                    };
                    let subpath = if is_commander {
                        None
                    } else {
                        git_cache.subpath_for_pane(pane)
                    };
                    let title = if is_commander {
                        commander_chrome_title_label()
                    } else {
                        pane.chrome_title_label()
                    };
                    render_panel_title_chrome(
                        f,
                        pane_area,
                        &title,
                        folder_name.as_deref(),
                        git_summary.as_ref(),
                        subpath.as_deref(),
                        is_commander,
                        title_row_style,
                        title_row_selected,
                        edge_style,
                        theme,
                        true,
                        is_maximized,
                        !placement.exposed.right,
                        !placement.exposed.bottom,
                        !placement.exposed.left,
                    );

                    let inner = pane_inner_area(pane_area, placement.exposed, is_commander);
                    if inner.width > 0 && inner.height > 0 {
                        if in_swap_preview {
                            render_pane_swap_drop_overlay(f, inner, theme, title_row_style);
                            continue;
                        }

                        if is_commander {
                            let metrics = render_commander_panel_content(
                                f,
                                app,
                                inner,
                                theme,
                                focused,
                                modal_is_none,
                            );
                            commander_chat_metrics = Some(metrics);
                            continue;
                        }

                        let content_area = clip_rect_to_frame(inner, f.size());

                        let pane_view =
                            pane_view_for_render(pane.styled_view(selection), theme, focused);
                        render_pane_text(
                            f,
                            pane_view,
                            content_area,
                            Style::default().bg(bg_color(theme)),
                        );

                        if modal_is_none && focused {
                            if let Some((x, y)) = pane.cursor_position_in(content_area) {
                                f.set_cursor(x, y);
                            }
                        }
                    }
                }

                if let Some(modal) = app.modal.as_ref() {
                    match modal {
                        ui::Modal::Help => {
                            render_help_modal(f, f.size(), theme, app.debug_container_boxes())
                        }
                        ui::Modal::Theme => {
                            render_theme_modal(f, f.size(), app.theme_preview_index, theme)
                        }
                        ui::Modal::NewPanePicker {
                            pane_id,
                            name,
                            name_error,
                            cursor,
                            name_selected,
                            agent_index,
                            ..
                        } => {
                            let (pane_area, anchor_title) = app
                                .pane_placements(App::content_area(f.size()))
                                .into_iter()
                                .find(|placement| placement.pane_id == *pane_id)
                                .and_then(|placement| {
                                    app.panes
                                        .iter()
                                        .find(|pane| pane.id == *pane_id)
                                        .map(|pane| {
                                            (
                                                placement.area,
                                                pane.chrome_title_label(),
                                            )
                                        })
                                })
                                .unwrap_or_else(|| {
                                    let area = App::content_area(f.size());
                                    (area, "Pane".to_string())
                                });
                            let title_bar_height =
                                pane_title_bar_height(app.pane_is_commander(*pane_id));
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_new_pane_picker_modal(
                                f,
                                pane_area,
                                &anchor_title,
                                title_bar_height,
                                theme,
                                name,
                                name_error.as_deref(),
                                *cursor,
                                *name_selected,
                                *agent_index,
                                &availability,
                            );
                        }
                        ui::Modal::PanelSettings {
                            pane_id,
                            name,
                            name_error,
                            agent_index,
                            focus,
                            ..
                        } => {
                            let (pane_area, anchor_title) = app
                                .pane_placements(App::content_area(f.size()))
                                .into_iter()
                                .find(|placement| placement.pane_id == *pane_id)
                                .and_then(|placement| {
                                    app.panes
                                        .iter()
                                        .find(|pane| pane.id == *pane_id)
                                        .map(|pane| {
                                            (
                                                placement.area,
                                                pane.chrome_title_label(),
                                            )
                                        })
                                })
                                .unwrap_or_else(|| {
                                    let area = App::content_area(f.size());
                                    (area, "Pane".to_string())
                                });
                            let title_bar_height =
                                pane_title_bar_height(app.pane_is_commander(*pane_id));
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_panel_settings_modal(
                                f,
                                pane_area,
                                &anchor_title,
                                title_bar_height,
                                theme,
                                name,
                                name_error.as_deref(),
                                *agent_index,
                                *focus,
                                &availability,
                            )
                        }
                        ui::Modal::WorkspaceSettings {
                            name,
                            name_error,
                            cursor,
                            action_index,
                            ..
                        } => render_workspace_settings_modal(
                            f,
                            f.size(),
                            theme,
                            name,
                            name_error.as_deref(),
                            *cursor,
                            *action_index,
                        ),
                        ui::Modal::WorktreePicker {
                            pane_id,
                            folder_name,
                            git_summary,
                            repo_root,
                            target_branch,
                            entries,
                            entry_summaries,
                            entry_snapshots,
                            entry_states,
                            current_path,
                            selected_index,
                            list_column,
                            focus,
                            submodal,
                            delete_target_index,
                            delete_action_index,
                            ..
                        } => {
                            let (pane_area, chrome_title) = app
                                .pane_placements(App::content_area(f.size()))
                                .into_iter()
                                .find(|placement| placement.pane_id == *pane_id)
                                .and_then(|placement| {
                                    app.panes
                                        .iter()
                                        .find(|pane| pane.id == *pane_id)
                                        .map(|pane| {
                                            (
                                                placement.area,
                                                pane.chrome_title_label(),
                                            )
                                        })
                                })
                                .unwrap_or_else(|| {
                                    let area = App::content_area(f.size());
                                    (area, "Pane".to_string())
                                });
                            render_worktree_picker_modal(
                                f,
                                pane_area,
                                &chrome_title,
                                folder_name,
                                git_summary,
                                theme,
                                entries,
                                entry_summaries,
                                entry_snapshots,
                                entry_states,
                                repo_root,
                                target_branch,
                                current_path.as_deref(),
                                *selected_index,
                                *list_column,
                                *focus,
                                submodal,
                                *delete_target_index,
                                *delete_action_index,
                            );
                        }
                    }
                }
            })?;
            if let Some((viewport, total)) = commander_chat_metrics {
                app.set_commander_chat_metrics(viewport, total);
            }
            dirty = false;
        }

        let should_show_cursor = app.modal.is_none()
            || matches!(
                app.modal,
                Some(ui::Modal::NewPanePicker { .. })
                    | Some(ui::Modal::PanelSettings {
                        focus: ui::PanelSettingsFocus::Name,
                        ..
                    })
                    | Some(ui::Modal::WorkspaceSettings { .. })
                    | Some(ui::Modal::WorktreePicker {
                        submodal:
                            crate::worktree_ui::WorktreeSubmodal::NewWorktree { .. }
                            | crate::worktree_ui::WorktreeSubmodal::RunAgent { .. }
                            | crate::worktree_ui::WorktreeSubmodal::CommitMessage { .. },
                        ..
                    })
            );
        if should_show_cursor != cursor_visible {
            if should_show_cursor {
                execute!(terminal.backend_mut(), Show)?;
            } else {
                execute!(terminal.backend_mut(), Hide)?;
            }
            cursor_visible = should_show_cursor;
        }

        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) => {
                    app.handle_key(key, size)?;
                    dirty = true;
                }
                Event::Mouse(mouse) => {
                    if matches!(mouse.kind, MouseEventKind::Moved) {
                        let now = std::time::Instant::now();
                        if last_mouse_move_at
                            .is_some_and(|last| now.duration_since(last) < MOUSE_MOVE_DEBOUNCE)
                        {
                            continue;
                        }
                        last_mouse_move_at = Some(now);
                        last_mouse_position = Some((mouse.column, mouse.row));
                        app.handle_mouse(mouse, size)?;
                    } else {
                        last_mouse_position = Some((mouse.column, mouse.row));
                        app.handle_mouse(mouse, size)?;
                        dirty = true;
                    }
                }
                Event::Paste(text) => {
                    app.handle_paste(text)?;
                    dirty = true;
                }
                Event::Resize(_, _) => {
                    dirty = true;
                }
                _ => {}
            }
        }

        let desired_pointer_shape = if app.mouse_capture_enabled() {
            last_mouse_position
                .map(|(x, y)| app.pointer_shape_at(size, x, y))
                .unwrap_or(MousePointerShape::Default)
        } else {
            MousePointerShape::Default
        };
        if desired_pointer_shape != mouse_pointer_shape {
            set_mouse_pointer_shape(terminal.backend_mut(), desired_pointer_shape)?;
            mouse_pointer_shape = desired_pointer_shape;
        }

        if app.mouse_capture_enabled() != mouse_capture_enabled {
            if app.mouse_capture_enabled() {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
            }
            mouse_capture_enabled = app.mouse_capture_enabled();
        }
    }

    Ok(())
}

fn set_mouse_pointer_shape(out: &mut impl io::Write, shape: MousePointerShape) -> io::Result<()> {
    let name = match shape {
        MousePointerShape::Default => "default",
        MousePointerShape::HorizontalResize => "ew-resize",
        MousePointerShape::VerticalResize => "ns-resize",
    };
    write!(out, "\x1b]22;{}\x1b\\", name)?;
    out.flush()
}

fn render_pane_swap_drop_overlay(f: &mut Frame<'_>, area: Rect, theme: Theme, style: Style) {
    f.render_widget(
        Paragraph::new("").style(Style::default().fg(theme.foreground).bg(bg_color(theme))),
        area,
    );

    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .style(Style::default().bg(bg_color(theme))),
        area,
    );

    let message_y = area.y + area.height.saturating_sub(1) / 2;
    let message_area = Rect {
        x: area.x,
        y: message_y,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new("Drop to swap")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.foreground).bg(bg_color(theme))),
        message_area,
    );
}


/// Render terminal text one row at a time. A single multi-line `Paragraph` can
/// write past the bottom of the frame when the rect is taller than the remaining
/// space or ratatui expands wrapped rows.
fn render_pane_text(f: &mut Frame<'_>, text: Text<'static>, area: Rect, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_rows = area.height as usize;
    for (row, line) in text.lines.into_iter().enumerate().take(max_rows) {
        let row_area = Rect {
            x: area.x,
            y: area.y.saturating_add(row as u16),
            width: area.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(line).style(style), row_area);
    }
}

fn pane_view_for_render(view: Text<'static>, theme: Theme, focused: bool) -> Text<'static> {
    if theme.passthrough {
        return view;
    }

    if focused {
        return theme_text(view, theme);
    }

    monochrome_text(view, theme)
}

fn theme_text(text: Text<'static>, theme: Theme) -> Text<'static> {
    transform_text(text, |style| theme_style(style, theme))
}

fn monochrome_text(text: Text<'static>, theme: Theme) -> Text<'static> {
    transform_text(text, |style| monochrome_style(style, theme))
}

fn transform_text(text: Text<'static>, mut map_style: impl FnMut(Style) -> Style) -> Text<'static> {
    Text {
        lines: text
            .lines
            .into_iter()
            .map(|line| transform_line(line, &mut map_style))
            .collect(),
        style: map_style(text.style),
        alignment: text.alignment,
    }
}

fn transform_line(
    line: Line<'static>,
    map_style: &mut impl FnMut(Style) -> Style,
) -> Line<'static> {
    Line {
        spans: line
            .spans
            .into_iter()
            .map(|span| {
                let style = map_style(span.style);
                span.patch_style(style)
            })
            .collect(),
        style: map_style(line.style),
        alignment: line.alignment,
    }
}

fn monochrome_style(style: Style, theme: Theme) -> Style {
    recolor_style(style, theme, PaneColorMode::Monochrome)
}

fn theme_style(style: Style, theme: Theme) -> Style {
    recolor_style(style, theme, PaneColorMode::Palette)
}

#[derive(Clone, Copy)]
enum PaneColorMode {
    Palette,
    Monochrome,
}

fn recolor_style(style: Style, theme: Theme, mode: PaneColorMode) -> Style {
    let fg = match style.fg {
        Some(Color::Reset) | None => Some(default_foreground(theme, mode)),
        Some(color) => Some(recolor_color(color, theme, mode, true)),
    };
    let bg = match style.bg {
        Some(Color::Reset) | None => None,
        Some(color) => Some(recolor_color(color, theme, mode, false)),
    };

    let mut out = Style::default();
    if let Some(color) = fg {
        out = out.fg(color);
    }
    if let Some(color) = bg {
        out = out.bg(color);
    }
    out = out.add_modifier(style.add_modifier);
    out = out.remove_modifier(style.sub_modifier);
    out
}

fn default_foreground(theme: Theme, mode: PaneColorMode) -> Color {
    match mode {
        PaneColorMode::Palette => theme.foreground,
        PaneColorMode::Monochrome => theme.muted,
    }
}

fn recolor_color(color: Color, theme: Theme, mode: PaneColorMode, is_foreground: bool) -> Color {
    match mode {
        PaneColorMode::Monochrome => {
            if is_foreground {
                theme.muted
            } else {
                Color::Reset
            }
        }
        PaneColorMode::Palette => match color {
            Color::Black => palette_color(theme, 0),
            Color::Red => palette_color(theme, 1),
            Color::Green => palette_color(theme, 2),
            Color::Yellow => palette_color(theme, 3),
            Color::Blue => palette_color(theme, 4),
            Color::Magenta => palette_color(theme, 5),
            Color::Cyan => palette_color(theme, 6),
            Color::Gray => palette_color(theme, 7),
            Color::DarkGray => palette_color(theme, 8),
            Color::LightRed => palette_color(theme, 9),
            Color::LightGreen => palette_color(theme, 10),
            Color::LightYellow => palette_color(theme, 11),
            Color::LightBlue => palette_color(theme, 12),
            Color::LightMagenta => palette_color(theme, 13),
            Color::LightCyan => palette_color(theme, 14),
            Color::White => palette_color(theme, 15),
            Color::Indexed(index) if (index as usize) < theme.palette.len() => {
                palette_color(theme, index as usize)
            }
            Color::Indexed(index) => nearest_theme_color(indexed_color_to_rgb(index), theme),
            Color::Rgb(_, _, _) => nearest_theme_color(color_to_rgb(color), theme),
            Color::Reset => default_foreground(theme, mode),
        },
    }
}

fn palette_color(theme: Theme, index: usize) -> Color {
    theme
        .palette
        .get(index)
        .copied()
        .unwrap_or(theme.foreground)
}

fn nearest_theme_color(color: (u8, u8, u8), theme: Theme) -> Color {
    theme
        .palette
        .iter()
        .copied()
        .min_by_key(|candidate| color_distance_sq(color, color_to_rgb(*candidate)))
        .unwrap_or(theme.foreground)
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Reset => (0, 0, 0),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_color_to_rgb(index),
    }
}

fn indexed_color_to_rgb(index: u8) -> (u8, u8, u8) {
    const ANSI_16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];

    match index {
        0..=15 => ANSI_16[index as usize],
        16..=231 => {
            let idx = index - 16;
            let r = idx / 36;
            let g = (idx % 36) / 6;
            let b = idx % 6;
            (cube_component(r), cube_component(g), cube_component(b))
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            (level, level, level)
        }
    }
}

fn cube_component(component: u8) -> u8 {
    match component {
        0 => 0,
        n => 55 + n * 40,
    }
}

fn color_distance_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = i32::from(a.0) - i32::from(b.0);
    let dg = i32::from(a.1) - i32::from(b.1);
    let db = i32::from(a.2) - i32::from(b.2);
    (dr * dr + dg * dg + db * db) as u32
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChatSide {
    Left,
    Right,
}

#[derive(Clone)]
struct ChatRenderLine {
    side: ChatSide,
    text: String,
    fg: Color,
    bg: Color,
    is_tail: bool,
}

fn render_commander_panel_content(
    f: &mut Frame<'_>,
    app: &App,
    inner: Rect,
    theme: Theme,
    focused: bool,
    modal_is_none: bool,
) -> (u16, usize) {
    let content_height = inner.height;
    let commander_input = app.commander_input();
    let clamped_cursor = app.commander_cursor().min(commander_input.chars().count());
    let full_input = format!("> {}", commander_input);
    let cursor_char_index = 2 + clamped_cursor;
    let (wrapped_lines, cursor_row, cursor_col) =
        hard_wrap_with_cursor(&full_input, cursor_char_index, inner.width);
    let total_input_lines = wrapped_lines.len() as u16;
    let input_height = total_input_lines.clamp(1, content_height);
    let input_top = inner.bottom().saturating_sub(input_height);
    let visible_start = total_input_lines.saturating_sub(input_height);
    let visible_input = wrapped_lines[visible_start as usize..].join("\n");

    let logs_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: input_top.saturating_sub(inner.y),
    };

    let chat_metrics = if logs_area.height > 0 {
        let [top_third, middle_third, bottom_third] = split_rect_into_thirds(logs_area);

        if middle_third.height > 0 {
            if let Some(frame) = app.commander_palette_video_frame() {
                render_ascii_character_overlay(f, middle_third, frame, theme);
            }
        }

        let chat_area = if bottom_third.height > 0 {
            bottom_third
        } else if middle_third.height > 0 {
            middle_third
        } else {
            top_third
        };
        if chat_area.height > 0 && chat_area.y > logs_area.y && logs_area.width > 0 {
            let divider = "─".repeat(logs_area.width as usize);
            f.render_widget(
                Paragraph::new(divider)
                    .style(Style::default().fg(theme.muted).bg(bg_color(theme))),
                Rect {
                    x: logs_area.x,
                    y: chat_area.y.saturating_sub(1),
                    width: logs_area.width,
                    height: 1,
                },
            );
        }
        let chat_lines = build_commander_chat_lines(app.commander_history(), chat_area.width, theme);
        render_commander_chat_overlay(
            f,
            chat_area,
            &chat_lines,
            app.commander_chat_offset_from_bottom(),
            theme,
        );
        (chat_area.height, chat_lines.len())
    } else {
        (0, 0)
    };

    let input_area = Rect {
        x: inner.x,
        y: input_top,
        width: inner.width,
        height: input_height,
    };
    let input_style = if focused {
        Style::default().fg(theme.accent).bg(bg_color(theme))
    } else {
        Style::default().fg(theme.foreground).bg(bg_color(theme))
    };
    f.render_widget(Paragraph::new(visible_input).style(input_style), input_area);

    if modal_is_none && focused {
        let visible_cursor_row = cursor_row.saturating_sub(visible_start as usize) as u16;
        let cursor_x = input_area
            .x
            .saturating_add((cursor_col as u16).min(input_area.width.saturating_sub(1)));
        let cursor_y = input_area
            .y
            .saturating_add(visible_cursor_row.min(input_area.height.saturating_sub(1)));
        f.set_cursor(cursor_x, cursor_y);
    }

    chat_metrics
}

fn render_commander_chat_overlay(
    f: &mut Frame<'_>,
    area: Rect,
    lines: &[ChatRenderLine],
    scroll_offset_from_bottom: usize,
    theme: Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_lines = area.height as usize;
    let end = lines.len().saturating_sub(scroll_offset_from_bottom);
    let start = end.saturating_sub(max_lines);
    let visible = lines
        .iter()
        .skip(start)
        .take(max_lines)
        .cloned()
        .collect::<Vec<_>>();

    for (row, line) in visible.into_iter().enumerate() {
        let y = area.y.saturating_add(row as u16);
        if y >= area.bottom() {
            break;
        }
        let text_width = line.text.chars().count().min(area.width as usize).max(1) as u16;
        let x = match line.side {
            ChatSide::Left => area.x,
            ChatSide::Right => area.right().saturating_sub(text_width),
        };
        let style = if line.is_tail {
            Style::default().fg(line.bg).bg(bg_color(theme))
        } else {
            Style::default().fg(line.fg).bg(line.bg)
        };
        f.render_widget(
            Paragraph::new(line.text).style(style),
            Rect {
                x,
                y,
                width: text_width,
                height: 1,
            },
        );
    }
}

fn render_ascii_character_overlay(f: &mut Frame<'_>, area: Rect, frame: &str, theme: Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    for (row, line) in frame.lines().take(area.height as usize).enumerate() {
        let mut run = String::new();
        let mut run_start = 0usize;
        let mut col = 0usize;
        for ch in line.chars().take(area.width as usize) {
            if ch == ' ' {
                if !run.is_empty() {
                    f.render_widget(
                        Paragraph::new(run.clone()).style(style),
                        Rect {
                            x: area.x.saturating_add(run_start as u16),
                            y: area.y.saturating_add(row as u16),
                            width: run.chars().count() as u16,
                            height: 1,
                        },
                    );
                    run.clear();
                }
            } else {
                if run.is_empty() {
                    run_start = col;
                }
                run.push(ch);
            }
            col += 1;
        }
        if !run.is_empty() {
            f.render_widget(
                Paragraph::new(run).style(style),
                Rect {
                    x: area.x.saturating_add(run_start as u16),
                    y: area.y.saturating_add(row as u16),
                    width: col.saturating_sub(run_start) as u16,
                    height: 1,
                },
            );
        }
    }
}

fn build_commander_chat_lines(history: &[String], width: u16, theme: Theme) -> Vec<ChatRenderLine> {
    let total_width = width.max(1) as usize;
    let (user_bg, user_fg, commander_bg, commander_fg) = if theme.passthrough {
        (Color::Blue, Color::White, Color::DarkGray, Color::White)
    } else {
        (
            theme.accent,
            bg_color(theme),
            theme.muted,
            theme.foreground,
        )
    };

    let mut out = Vec::<ChatRenderLine>::new();
    let max_inner_width = total_width.saturating_sub(6).max(8);
    let mut last_side: Option<ChatSide> = None;

    for entry in history {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (side, speaker, text, bubble_bg, bubble_fg) =
            if let Some(text) = trimmed.strip_prefix("You: ") {
                (ChatSide::Right, "You", text.trim(), user_bg, user_fg)
            } else if let Some(text) = trimmed.strip_prefix("Commander: ") {
                (
                    ChatSide::Left,
                    "Commander",
                    text.trim(),
                    commander_bg,
                    commander_fg,
                )
            } else {
                out.push(ChatRenderLine {
                    side: ChatSide::Left,
                    text: trimmed.to_string(),
                    fg: theme.muted,
                    bg: bg_color(theme),
                    is_tail: false,
                });
                continue;
            };

        if text.is_empty() {
            continue;
        }

        if last_side.is_some() && last_side != Some(side) {
            out.push(ChatRenderLine {
                side: ChatSide::Left,
                text: String::new(),
                fg: theme.foreground,
                bg: bg_color(theme),
                is_tail: false,
            });
        }
        out.push(ChatRenderLine {
            side,
            text: format!("{speaker}:"),
            fg: theme.muted,
            bg: bg_color(theme),
            is_tail: false,
        });

        let wrapped = hard_wrap_text(text, max_inner_width);
        let inner_width = wrapped
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);

        for line in wrapped {
            let padding = inner_width.saturating_sub(line.chars().count());
            let bubble_text = format!(" {}{} ", line, " ".repeat(padding));
            out.push(ChatRenderLine {
                side,
                text: bubble_text,
                fg: bubble_fg,
                bg: bubble_bg,
                is_tail: false,
            });
        }

        let tail_char = match side {
            ChatSide::Right => "◥",
            ChatSide::Left => "◤",
        };
        out.push(ChatRenderLine {
            side,
            text: tail_char.to_string(),
            fg: bubble_bg,
            bg: bg_color(theme),
            is_tail: true,
        });
        out.push(ChatRenderLine {
            side: ChatSide::Left,
            text: String::new(),
            fg: theme.foreground,
            bg: bg_color(theme),
            is_tail: false,
        });
        last_side = Some(side);
    }

    out
}

fn hard_wrap_text(text: &str, width: usize) -> Vec<String> {
    let wrap_width = width.max(1);
    let mut lines = vec![String::new()];
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(String::new());
            col = 0;
            continue;
        }
        if col >= wrap_width {
            lines.push(String::new());
            col = 0;
        }
        if let Some(line) = lines.last_mut() {
            line.push(ch);
        }
        col += 1;
    }
    lines
}

fn split_rect_into_thirds(area: Rect) -> [Rect; 3] {
    if area.height == 0 {
        return [area, area, area];
    }

    let base = area.height / 3;
    let remainder = area.height % 3;
    let h0 = base;
    let h1 = base;
    let h2 = base + remainder;
    let y1 = area.y.saturating_add(h0);
    let y2 = y1.saturating_add(h1);

    [
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: h0,
        },
        Rect {
            x: area.x,
            y: y1,
            width: area.width,
            height: h1,
        },
        Rect {
            x: area.x,
            y: y2,
            width: area.width,
            height: h2,
        },
    ]
}

fn hard_wrap_with_cursor(
    text: &str,
    cursor_char_index: usize,
    width: u16,
) -> (Vec<String>, usize, usize) {
    let wrap_width = width.max(1) as usize;
    let mut lines = vec![String::new()];
    let mut row = 0usize;
    let mut col = 0usize;
    let mut chars_seen = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;

    let mut set_cursor_if_match = |seen: usize, row: usize, col: usize| {
        if seen == cursor_char_index {
            cursor_row = row;
            cursor_col = col;
        }
    };
    set_cursor_if_match(0, row, col);

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(String::new());
            row += 1;
            col = 0;
        } else {
            if col >= wrap_width {
                lines.push(String::new());
                row += 1;
                col = 0;
            }
            if let Some(line) = lines.last_mut() {
                line.push(ch);
            }
            col += 1;
        }

        chars_seen += 1;
        set_cursor_if_match(chars_seen, row, col);
    }

    if cursor_char_index > chars_seen {
        cursor_row = row;
        cursor_col = col;
    }

    (lines, cursor_row, cursor_col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Stylize;

    const TEST_PALETTE: &[Color] = &[
        Color::Rgb(20, 20, 24),
        Color::Rgb(220, 80, 90),
        Color::Rgb(90, 200, 120),
        Color::Rgb(230, 200, 120),
        Color::Rgb(90, 160, 240),
        Color::Rgb(220, 120, 220),
        Color::Rgb(80, 200, 210),
        Color::Rgb(220, 220, 230),
        Color::Rgb(120, 120, 130),
        Color::Rgb(255, 130, 150),
        Color::Rgb(140, 220, 150),
        Color::Rgb(255, 220, 120),
        Color::Rgb(120, 180, 255),
        Color::Rgb(240, 160, 240),
        Color::Rgb(120, 230, 230),
        Color::Rgb(255, 255, 255),
    ];
    const RESET_PALETTE: &[Color] = &[Color::Reset; 16];

    fn test_theme() -> Theme {
        Theme {
            name: "Test",
            background: Color::Reset,
            foreground: Color::Rgb(220, 220, 230),
            muted: Color::Rgb(120, 120, 130),
            accent: Color::Rgb(80, 140, 220),
            passthrough: false,
            palette: TEST_PALETTE,
        }
    }

    #[test]
    fn monochrome_text_overrides_span_colors_without_touching_modifiers() {
        let theme = test_theme();

        let text = Text {
            lines: vec![Line {
                spans: vec![
                    "hello".fg(Color::Red).bg(Color::Blue).bold(),
                    "world".fg(Color::Green).bg(Color::Magenta).italic(),
                ],
                style: Style::default().fg(Color::Yellow).bg(Color::Cyan),
                alignment: None,
            }],
            style: Style::default().fg(Color::LightRed).bg(Color::LightBlue),
            alignment: None,
        };

        let rendered = monochrome_text(text, theme);
        assert_eq!(rendered.style.fg, Some(theme.muted));
        assert_eq!(rendered.style.bg, Some(Color::Reset));
        assert_eq!(rendered.lines[0].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].style.bg, Some(Color::Reset));
        assert_eq!(rendered.lines[0].spans[0].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].spans[0].style.bg, Some(Color::Reset));
        assert!(rendered.lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(rendered.lines[0].spans[1].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].spans[1].style.bg, Some(Color::Reset));
        assert!(rendered.lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
    }

    #[test]
    fn focused_text_remaps_colors_into_theme_palette() {
        let theme = test_theme();

        let text = Text {
            lines: vec![Line {
                spans: vec![
                    "blue".fg(Color::Blue),
                    "magenta".fg(Color::Magenta),
                    "rgb".fg(Color::Rgb(250, 80, 250)),
                ],
                style: Style::default(),
                alignment: None,
            }],
            style: Style::default(),
            alignment: None,
        };

        let rendered = theme_text(text, theme);
        assert_eq!(rendered.lines[0].spans[0].style.fg, Some(theme.palette[4]));
        assert_eq!(rendered.lines[0].spans[1].style.fg, Some(theme.palette[5]));
        assert!(matches!(
            rendered.lines[0].spans[2].style.fg,
            Some(color) if theme.palette.contains(&color)
        ));
        assert!(rendered.lines[0]
            .spans
            .iter()
            .all(|span| span.style.bg.is_none()));
    }

    #[test]
    fn render_pane_text_fills_exact_height_without_panicking() {
        use ratatui::backend::TestBackend;

        let height = 37u16;
        let width = 209u16;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let lines: Vec<Line<'_>> = (0..height)
            .map(|row| Line::from(format!("row {row}")))
            .collect();
        let text = Text::from(lines);

        terminal
            .draw(|frame| {
                render_pane_text(
                    frame,
                    text,
                    Rect::new(0, 0, width, height),
                    Style::default(),
                );
            })
            .unwrap();
    }

    #[test]
    fn render_pane_text_clips_rect_that_extends_past_frame_bottom() {
        use ratatui::backend::TestBackend;

        let frame = Rect::new(0, 0, 209, 37);
        let area = Rect::new(0, 1, 209, 37);
        let lines: Vec<Line<'_>> = (0..37)
            .map(|row| Line::from(format!("row {row}")))
            .collect();
        let text = Text::from(lines);
        let mut terminal = Terminal::new(TestBackend::new(frame.width, frame.height)).unwrap();

        terminal
            .draw(|f| {
                let clipped = clip_rect_to_frame(area, f.size());
                render_pane_text(f, text, clipped, Style::default());
            })
            .unwrap();
    }

    #[test]
    fn passthrough_theme_keeps_original_colors() {
        let theme = Theme {
            name: "None",
            background: Color::Reset,
            foreground: Color::Reset,
            muted: Color::Reset,
            accent: Color::Reset,
            passthrough: true,
            palette: RESET_PALETTE,
        };

        let text = Text {
            lines: vec![Line {
                spans: vec!["blue".fg(Color::Blue).bg(Color::Magenta)],
                style: Style::default().fg(Color::Yellow).bg(Color::Cyan),
                alignment: None,
            }],
            style: Style::default().fg(Color::Red).bg(Color::Green),
            alignment: None,
        };

        assert_eq!(pane_view_for_render(text.clone(), theme, true), text);
        assert_eq!(pane_view_for_render(text.clone(), theme, false), text);
    }

}
