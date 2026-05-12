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

use anyhow::Context;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, Wrap},
    Frame, Terminal,
};

use app::App;
use layout::{pane_borders, pane_inner_area, pane_title_bar_area, pane_title_y};
use ui::{
    render_close_confirm_modal, render_default_agent_button, render_default_agent_dropdown,
    render_help_modal, render_panel_settings_modal, render_settings_button, render_theme_modal,
    AGENT_PRESETS,
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

    let _ = layout::save_persisted_layout(
        &app.layout,
        app.focused,
        app.default_agent_index,
        &app.panes,
    );

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

fn truncate_to_width(text: &str, width: usize) -> String {
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

                render_settings_button(f, f.size(), theme);
                let default_label = AGENT_PRESETS
                    .get(app.default_agent_index)
                    .map(|p| p.label)
                    .unwrap_or(AGENT_PRESETS[0].label);
                render_default_agent_button(
                    f,
                    f.size(),
                    theme,
                    default_label,
                    matches!(app.modal, Some(ui::Modal::DefaultAgent { .. })),
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
                let modal_is_none = app.modal.is_none();
                let focused_pane_id = app.focused;

                for placement in placements {
                    let focused = placement.pane_id == focused_pane_id;
                    let in_resize_preview = preview_pane_ids
                        .as_ref()
                        .is_some_and(|pane_ids| pane_ids.contains(&placement.pane_id));
                    let Some(pane_index) = app
                        .panes
                        .iter()
                        .position(|pane| pane.id == placement.pane_id)
                    else {
                        continue;
                    };
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

                    let chrome_style = if focused || in_resize_preview {
                        Style::default()
                            .fg(theme.accent)
                            .bg(theme.background)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.muted).bg(theme.background)
                    };

                    let block = Block::default()
                        .borders(pane_borders(placement.exposed))
                        .style(Style::default().bg(theme.background))
                        .border_style(chrome_style);
                    f.render_widget(block, pane_area);
                    if focused {
                        render_fancy_selected_border(f, pane_area, chrome_style);
                    }

                    let title_bar = pane_title_bar_area(pane_area);
                    if title_bar.width > 0 && title_bar.height > 0 {
                        f.render_widget(
                            Paragraph::new("").style(Style::default().bg(theme.title_bar)),
                            title_bar,
                        );
                    }

                    let title_y = pane_title_y(pane_area);
                    if title_y < pane_area.bottom() {
                        let title_bar_width = pane_area.width.saturating_sub(2);
                        if title_bar_width > 0 {
                            let preview = if in_resize_preview { " resizing" } else { "" };
                            let title = format!("{}{}", pane.title, preview);
                            let title_text = if focused {
                                truncate_to_width(&format!("▶ {title} ◀"), title_bar_width as usize)
                            } else {
                                truncate_to_width(&title, title_bar_width as usize)
                            };
                            f.render_widget(
                                Paragraph::new(title_text)
                                    .alignment(Alignment::Center)
                                    .style(chrome_style.bg(theme.title_bar)),
                                Rect {
                                    x: pane_area.x.saturating_add(1),
                                    y: title_y,
                                    width: title_bar_width,
                                    height: 1,
                                },
                            );
                        }

                        if pane_area.width >= 9 {
                            f.render_widget(
                                Paragraph::new("[=]").style(chrome_style.bg(theme.title_bar)),
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
                                Paragraph::new("🗙").style(chrome_style.bg(theme.title_bar)),
                                Rect {
                                    x: pane_area.right().saturating_sub(4),
                                    y: title_y,
                                    width: 3,
                                    height: 1,
                                },
                            );
                        }
                    }

                    let inner = pane_inner_area(pane_area, placement.exposed);
                    if inner.width > 0 && inner.height > 0 {
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

                        let paragraph = Paragraph::new(pane.styled_view())
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
                        ui::Modal::DefaultAgent { agent_index } => {
                            render_default_agent_dropdown(f, f.size(), theme, *agent_index);
                        }
                        ui::Modal::PanelSettings {
                            name,
                            agent_index,
                            focus,
                            ..
                        } => render_panel_settings_modal(
                            f,
                            f.size(),
                            theme,
                            name,
                            *agent_index,
                            *focus,
                        ),
                        ui::Modal::CloseConfirm { .. } => {
                            render_close_confirm_modal(f, f.size(), theme)
                        }
                    }
                }
            })?;
            dirty = false;
        }

        let should_show_cursor = app.modal.is_none()
            || matches!(
                app.modal,
                Some(ui::Modal::PanelSettings {
                    focus: ui::PanelSettingsFocus::Name,
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
                    app.handle_mouse(mouse, size)?;
                    dirty = true;
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

fn render_fancy_selected_border(f: &mut Frame<'_>, area: Rect, style: Style) {
    if area.width < 2 || area.height < 2 {
        return;
    }

    let buf = f.buffer_mut();
    let x0 = area.x;
    let y0 = area.y;
    let x1 = area.right().saturating_sub(1);
    let y1 = area.bottom().saturating_sub(1);

    buf.set_string(x0, y0, "╔", style);
    buf.set_string(x1, y0, "╗", style);
    buf.set_string(x0, y1, "╚", style);
    buf.set_string(x1, y1, "╝", style);

    for x in x0.saturating_add(1)..x1 {
        buf.set_string(x, y0, "═", style);
        buf.set_string(x, y1, "═", style);
    }

    for y in y0.saturating_add(1)..y1 {
        buf.set_string(x0, y, "║", style);
        buf.set_string(x1, y, "║", style);
    }
}
