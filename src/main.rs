mod app;
mod layout;
mod pane;
mod theme;
mod ui;
mod utils;

use std::{env, io, process::Command, time::Duration};

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
    widgets::{Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, Wrap},
    Frame, Terminal,
};

use app::{App, MousePointerShape};
use layout::{
    pane_borders, pane_inner_area, pane_title_bar_area, pane_title_chrome_reserve, pane_title_y,
    PANE_TITLE_LEFT_PADDING,
};
use theme::Theme;
use ui::{
    commander_item_area, render_help_modal, render_new_pane_picker_modal,
    render_panel_settings_modal, render_theme_modal, render_top_chrome,
    render_workspace_settings_modal, render_workspace_sidebar, truncate_to_width,
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
                f.render_widget(
                    Block::default().style(Style::default().bg(theme.background)),
                    f.size(),
                );

                render_top_chrome(f, f.size(), theme);
                let workspace_names = app.workspace_names();
                render_workspace_sidebar(
                    f,
                    App::workspace_sidebar_area(f.size()),
                    theme,
                    &workspace_names,
                    app.active_workspace_index(),
                    app.commander_focused(),
                    app.sidebar_workspace_focused(),
                    app.sidebar_add_button_focused(),
                );

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
                render_commander_sidebar_panel(f, app, theme, modal_is_none);

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
                    let mut chrome_style = Style::default().fg(border_color).bg(theme.background);
                    if focused || in_resize_preview || in_swap_preview {
                        chrome_style = chrome_style.add_modifier(Modifier::BOLD);
                    }

                    let block = Block::default()
                        .borders(pane_borders(placement.exposed))
                        .border_type(BorderType::Rounded)
                        .style(Style::default().bg(theme.background))
                        .border_style(chrome_style);
                    f.render_widget(block, pane_area);

                    let title_bar = pane_title_bar_area(pane_area);
                    let title_y = pane_title_y(pane_area);
                    if title_y < pane_area.bottom() {
                        let title_bar_width = title_bar.width;
                        if title_bar_width > PANE_TITLE_LEFT_PADDING {
                            let preview = if in_resize_preview { " resizing" } else { "" };
                            let title = format!("{}{}", pane.title, preview);
                            let title_max = title_bar_width
                                .saturating_sub(pane_title_chrome_reserve(pane_area.width))
                                .saturating_sub(PANE_TITLE_LEFT_PADDING + 1)
                                as usize;
                            let title_text = format!(" {} ", truncate_to_width(&title, title_max));
                            let title_slot_w =
                                title_bar_width.saturating_sub(PANE_TITLE_LEFT_PADDING);
                            f.render_widget(
                                Paragraph::new(title_text)
                                    .alignment(Alignment::Left)
                                    .style(chrome_style),
                                Rect {
                                    x: title_bar.x.saturating_add(PANE_TITLE_LEFT_PADDING),
                                    y: title_y,
                                    width: title_slot_w,
                                    height: 1,
                                },
                            );
                        }

                        if pane_area.width >= 11 {
                            let maximize_icon = if is_maximized { "🗗" } else { "⛶" };
                            f.render_widget(
                                Paragraph::new(format!(" {} ", maximize_icon)).style(chrome_style),
                                Rect {
                                    x: pane_area.right().saturating_sub(8),
                                    y: title_y,
                                    width: 3,
                                    height: 1,
                                },
                            );
                        }

                        if pane_area.width >= 6 {
                            f.render_widget(
                                Paragraph::new("🗙 ").style(chrome_style),
                                Rect {
                                    x: pane_area.right().saturating_sub(4),
                                    y: title_y,
                                    width: 2,
                                    height: 1,
                                },
                            );
                        }
                    }

                    let inner = pane_inner_area(pane_area, placement.exposed);
                    if inner.width > 0 && inner.height > 0 {
                        if in_swap_preview {
                            render_pane_swap_drop_overlay(f, inner, theme, chrome_style);
                            continue;
                        }

                        let scrollbar_needed =
                            inner.width > 1 && pane.needs_scrollbar(inner.width - 1, inner.height);
                        let content_area = if scrollbar_needed {
                            Rect {
                                x: inner.x,
                                y: inner.y,
                                width: inner.width.saturating_sub(1),
                                height: inner.height,
                            }
                        } else {
                            inner
                        };

                        let pane_view =
                            pane_view_for_render(pane.styled_view(selection), theme, focused);
                        let paragraph = Paragraph::new(pane_view)
                            .wrap(Wrap { trim: false })
                            .style(Style::default().bg(theme.background));
                        f.render_widget(paragraph, content_area);

                        if scrollbar_needed {
                            let scrollbar = Scrollbar::default()
                                .orientation(ScrollbarOrientation::VerticalRight)
                                .thumb_style(if focused {
                                    Style::default().fg(theme.accent)
                                } else {
                                    Style::default().fg(theme.muted)
                                });
                            let mut state =
                                pane.scrollbar_state(content_area.width, content_area.height);
                            f.render_stateful_widget(
                                scrollbar,
                                Rect {
                                    x: inner.right().saturating_sub(1),
                                    y: inner.y,
                                    width: 1,
                                    height: inner.height,
                                },
                                &mut state,
                            );
                        }

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
                            let container = app
                                .pane_placements(App::content_area(f.size()))
                                .into_iter()
                                .find(|placement| placement.pane_id == *pane_id)
                                .map(|placement| placement.area)
                                .unwrap_or(App::content_area(f.size()));
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_new_pane_picker_modal(
                                f,
                                container,
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
                            let availability = app.agent_availability_for_pane(*pane_id);
                            render_panel_settings_modal(
                                f,
                                f.size(),
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
        Paragraph::new("").style(Style::default().fg(theme.foreground).bg(theme.title_bar)),
        area,
    );

    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .style(Style::default().bg(theme.title_bar)),
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
            .style(Style::default().fg(theme.foreground).bg(theme.title_bar)),
        message_area,
    );
}

fn render_commander_sidebar_panel(
    f: &mut Frame<'_>,
    app: &mut App,
    theme: Theme,
    modal_is_none: bool,
) {
    let sidebar = App::workspace_sidebar_area(f.size());
    let commander_area = commander_item_area(sidebar);
    if commander_area.width < 3 || commander_area.height < 3 {
        return;
    }

    let inner = Block::default().borders(Borders::ALL).inner(commander_area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let status = if app.commander_busy() {
        "thinking..."
    } else {
        "ready"
    };
    let header_label = if status == "ready" {
        "Commander"
    } else {
        status
    };
    let header_fg = if app.commander_focused() {
        theme.accent
    } else {
        theme.muted
    };
    let header = Paragraph::new(format!(" {} ", header_label))
        .alignment(Alignment::Left)
        .style(Style::default().fg(header_fg).bg(theme.background));
    f.render_widget(
        header,
        Rect {
            x: commander_area.x.saturating_add(1),
            y: commander_area.y,
            width: commander_area.width.saturating_sub(2),
            height: 1,
        },
    );

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
    if logs_area.height > 0 {
        app.set_commander_palette_video_size(logs_area.height, logs_area.width);
        let frame = app.commander_palette_video_frame().map(str::to_string);
        if let Some(frame) = frame.as_deref() {
            render_ascii_character_overlay(f, logs_area, frame, theme);
        }
        render_commander_chat_overlay(f, logs_area, app.commander_history(), theme);
    }

    let input_area = Rect {
        x: inner.x,
        y: input_top,
        width: inner.width,
        height: input_height,
    };
    f.render_widget(
        Paragraph::new(visible_input).style(Style::default().fg(theme.accent).bg(theme.background)),
        input_area,
    );

    if modal_is_none && app.commander_focused() {
        let visible_cursor_row = cursor_row.saturating_sub(visible_start as usize) as u16;
        let cursor_x = input_area
            .x
            .saturating_add((cursor_col as u16).min(input_area.width.saturating_sub(1)));
        let cursor_y = input_area
            .y
            .saturating_add(visible_cursor_row.min(input_area.height.saturating_sub(1)));
        f.set_cursor(cursor_x, cursor_y);
    }
}

#[derive(Clone, Copy)]
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

fn render_commander_chat_overlay(f: &mut Frame<'_>, area: Rect, history: &[String], theme: Theme) {
    let lines = build_commander_chat_lines(history, area.width, theme);
    let max_lines = area.height as usize;
    let start = lines.len().saturating_sub(max_lines);
    let visible = lines.into_iter().skip(start).collect::<Vec<_>>();

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
            Style::default().fg(line.bg).bg(theme.background)
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
            theme.background,
            theme.title_bar,
            theme.foreground,
        )
    };

    let mut out = Vec::<ChatRenderLine>::new();
    let max_inner_width = total_width.saturating_sub(6).max(8);

    for entry in history {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (side, text, bubble_bg, bubble_fg) = if let Some(text) = trimmed.strip_prefix("You: ") {
            (ChatSide::Right, text.trim(), user_bg, user_fg)
        } else if let Some(text) = trimmed.strip_prefix("Commander: ") {
            (ChatSide::Left, text.trim(), commander_bg, commander_fg)
        } else {
            out.push(ChatRenderLine {
                side: ChatSide::Left,
                text: trimmed.to_string(),
                fg: theme.muted,
                bg: theme.background,
                is_tail: false,
            });
            continue;
        };

        if text.is_empty() {
            continue;
        }

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
            bg: theme.background,
            is_tail: true,
        });
        out.push(ChatRenderLine {
            side: ChatSide::Left,
            text: String::new(),
            fg: theme.foreground,
            bg: theme.background,
            is_tail: false,
        });
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
                theme.background
            }
        }
        PaneColorMode::Palette => match color {
            Color::Black => theme.palette[0],
            Color::Red => theme.palette[1],
            Color::Green => theme.palette[2],
            Color::Yellow => theme.palette[3],
            Color::Blue => theme.palette[4],
            Color::Magenta => theme.palette[5],
            Color::Cyan => theme.palette[6],
            Color::Gray => theme.palette[7],
            Color::DarkGray => theme.palette[8],
            Color::LightRed => theme.palette[9],
            Color::LightGreen => theme.palette[10],
            Color::LightYellow => theme.palette[11],
            Color::LightBlue => theme.palette[12],
            Color::LightMagenta => theme.palette[13],
            Color::LightCyan => theme.palette[14],
            Color::White => theme.palette[15],
            Color::Indexed(index) if index < 16 => theme.palette[index as usize],
            Color::Indexed(index) => nearest_theme_color(indexed_color_to_rgb(index), theme),
            Color::Rgb(_, _, _) => nearest_theme_color(color_to_rgb(color), theme),
            Color::Reset => default_foreground(theme, mode),
        },
    }
}

fn nearest_theme_color(color: (u8, u8, u8), theme: Theme) -> Color {
    theme
        .palette
        .into_iter()
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

    fn test_theme() -> Theme {
        Theme {
            name: "Test",
            background: Color::Rgb(20, 20, 24),
            foreground: Color::Rgb(220, 220, 230),
            muted: Color::Rgb(120, 120, 130),
            accent: Color::Rgb(80, 140, 220),
            title_bar: Color::Rgb(190, 160, 90),
            passthrough: false,
            palette: [
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
            ],
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
        assert_eq!(rendered.style.bg, Some(theme.background));
        assert_eq!(rendered.lines[0].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].style.bg, Some(theme.background));
        assert_eq!(rendered.lines[0].spans[0].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].spans[0].style.bg, Some(theme.background));
        assert!(rendered.lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(rendered.lines[0].spans[1].style.fg, Some(theme.muted));
        assert_eq!(rendered.lines[0].spans[1].style.bg, Some(theme.background));
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
            title_bar: Color::Reset,
            passthrough: true,
            palette: [Color::Reset; 16],
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
