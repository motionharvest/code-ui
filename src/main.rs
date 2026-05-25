mod app;
mod git_status;
mod layout;
mod pane;
mod theme;
mod ui;
mod utils;

use std::{
    env,
    io,
    process::Command,
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
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame, Terminal,
};

use app::{App, MousePointerShape};
use git_status::GitStatusCache;
use layout::{pane_borders, pane_inner_area};
use theme::Theme;
use ui::{
    commander_input_cursor_position, compute_top_bar_layout, pane_chrome_title_label,
    render_help_modal, render_new_pane_picker_modal, render_panel_settings_modal,
    render_panel_title_chrome, render_theme_modal, render_top_chrome,
    render_workspace_settings_modal, render_workspace_sidebar, bg_color,
};

fn main() -> anyhow::Result<()> {
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

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags,);
    let _ = set_mouse_pointer_shape(terminal.backend_mut(), MousePointerShape::Default);
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );
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
                    app.commander_focused(),
                    app.commander_input(),
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
                let preview_pane_ids = app.resize_preview_pane_ids().map(|ids| ids.to_vec());
                let swap_preview_target = app.pane_swap_preview_target();
                let modal_is_none = app.modal.is_none();
                let commander_focused = app.commander_focused();
                let focused_pane_id = (!commander_focused
                    && app.sidebar_workspace_focused().is_none()
                    && !app.sidebar_add_button_focused())
                .then_some(app.focused);
                if modal_is_none && commander_focused {
                    if let Some((x, y)) = commander_input_cursor_position(
                        top_layout,
                        app.commander_input(),
                        app.commander_cursor(),
                    ) {
                        f.set_cursor(x, y);
                    }
                }

                for placement in placements {
                    let focused = focused_pane_id == Some(placement.pane_id);
                    let in_resize_preview = preview_pane_ids
                        .as_ref()
                        .is_some_and(|pane_ids| pane_ids.contains(&placement.pane_id));
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

                    if pane_area.width == 0 || pane_area.height == 0 {
                        continue;
                    }

                    let border_color = if focused || in_resize_preview || in_swap_preview {
                        theme.accent
                    } else {
                        theme.muted
                    };
                    let chrome_style = Style::default().fg(border_color).bg(bg_color(theme));

                    let block = Block::default()
                        .borders(pane_borders(placement.exposed))
                        .border_type(BorderType::Rounded)
                        .style(Style::default().bg(bg_color(theme)))
                        .border_style(chrome_style);
                    f.render_widget(block, pane_area);
                    pane.mark_painted();

                    let git_summary = git_cache.summary_for_pane(pane);
                    let title = pane_chrome_title_label(&pane.title, &pane.command);
                    render_panel_title_chrome(
                        f,
                        pane_area,
                        &title,
                        git_summary.as_ref(),
                        chrome_style,
                        theme,
                        true,
                        is_maximized,
                    );

                    let inner = pane_inner_area(pane_area, placement.exposed);
                    if inner.width > 0 && inner.height > 0 {
                        if in_swap_preview {
                            render_pane_swap_drop_overlay(f, inner, theme, chrome_style);
                            continue;
                        }

                        let content_area = inner;

                        let pane_view =
                            pane_view_for_render(pane.styled_view(selection), theme, focused);
                        let paragraph = Paragraph::new(pane_view)
                            .wrap(Wrap { trim: false })
                            .style(Style::default().bg(bg_color(theme)));
                        f.render_widget(paragraph, content_area);

                        if modal_is_none && focused && !commander_focused {
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
                                                pane_chrome_title_label(&pane.title, &pane.command),
                                            )
                                        })
                                })
                                .unwrap_or_else(|| {
                                    let area = App::content_area(f.size());
                                    (area, "Pane".to_string())
                                });
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_new_pane_picker_modal(
                                f,
                                pane_area,
                                &anchor_title,
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
                                                pane_chrome_title_label(&pane.title, &pane.command),
                                            )
                                        })
                                })
                                .unwrap_or_else(|| {
                                    let area = App::content_area(f.size());
                                    (area, "Pane".to_string())
                                });
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_panel_settings_modal(
                                f,
                                pane_area,
                                &anchor_title,
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
                    }
                }
            })?;
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
