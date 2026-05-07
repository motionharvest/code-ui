mod app;
mod layout;
mod pane;
mod theme;
mod ui;
mod utils;

use std::{io, process::Command, time::Duration};

use anyhow::Context;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::Block,
    Terminal,
};

use app::App;
use ui::{render_help_modal, render_rename_modal, render_theme_modal};

fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let size = terminal.size()?;
    let mut app = App::new(size.height, size.width).context("failed to create panes")?;

    let res = run(&mut terminal, &mut app);
    let reload_requested = app.reload_requested;
    let _ = layout::save_persisted_layout(&app.layout, app.focused, &app.panes);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res?;
    if reload_requested {
        restart_via_script();
    }

    Ok(())
}

fn restart_via_script() {
    if let Err(err) = Command::new("./reload.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .spawn()
    {
        eprintln!("failed to restart with reload.sh: {err}");
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> anyhow::Result<()> {
    let mut cursor_visible = false;

    while app.running {
        let size = terminal.size()?;
        app.resize(size.height, size.width);
        app.tick();

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

            let placements = app.pane_placements(f.size());

            for placement in placements {
                let focused = placement.pane_id == app.focused;
                let Some(pane) = app.panes.iter().find(|pane| pane.id == placement.pane_id) else {
                    continue;
                };

                let title = if focused {
                    format!("{}*", pane.title)
                } else {
                    pane.title.clone()
                };

                let block = Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .title(title)
                    .style(Style::default().fg(theme.foreground).bg(theme.background))
                    .border_style(if focused {
                        Style::default().fg(theme.accent)
                    } else {
                        Style::default().fg(theme.muted)
                    });
                let inner = block.inner(placement.area);
                f.render_widget(block, placement.area);

                let content_area = if inner.width > 1 {
                    let parts = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Horizontal)
                        .constraints([ratatui::layout::Constraint::Min(1), ratatui::layout::Constraint::Length(1)])
                        .split(inner);

                    let paragraph = ratatui::widgets::Paragraph::new(pane.styled_view())
                        .wrap(ratatui::widgets::Wrap { trim: false })
                        .style(Style::default().bg(theme.background));
                    f.render_widget(paragraph, parts[0]);

                    let scrollbar = ratatui::widgets::Scrollbar::default()
                        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
                        .thumb_style(if focused {
                            Style::default().fg(theme.accent)
                        } else {
                            Style::default().fg(theme.muted)
                        });
                    let mut state = pane.scrollbar_state();
                    f.render_stateful_widget(scrollbar, parts[1], &mut state);
                    parts[0]
                } else {
                    let paragraph = ratatui::widgets::Paragraph::new(pane.styled_view())
                        .wrap(ratatui::widgets::Wrap { trim: false })
                        .style(Style::default().bg(theme.background));
                    f.render_widget(paragraph, inner);
                    inner
                };

                if app.modal.is_none() && focused {
                    if let Some((x, y)) = pane.cursor_position_in(content_area) {
                        f.set_cursor(x, y);
                    }
                }
            }

            if let Some(modal) = app.modal.as_ref() {
                match modal {
                    ui::Modal::Help => render_help_modal(f, f.size(), theme),
                    ui::Modal::Theme => render_theme_modal(f, f.size(), app.theme_preview_index, theme),
                    ui::Modal::Rename { input, .. } => render_rename_modal(f, f.size(), theme, input),
                }
            }
        })?;

        let should_show_cursor = app.modal.is_none() || matches!(app.modal, Some(ui::Modal::Rename { .. }));
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
                Event::Key(key) => app.handle_key(key, size)?,
                Event::Mouse(mouse) => app.handle_mouse(mouse, size)?,
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}
