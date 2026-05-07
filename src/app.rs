use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::{
    layout::{
        adjacent_overlap, load_persisted_layout, placement_is_adjacent, save_persisted_layout,
        ExposedSides, Node, Placement, SplitSide,
    },
    pane::Pane,
    theme::{load_persisted_theme_index, save_persisted_theme, Theme, THEMES},
    ui::{help_close_button_area, help_modal_area, rename_close_button_area, rename_modal_area, Modal},
    utils::{arrow_key_to_split_side, contains, is_close_pane_shortcut, key_to_bytes, shrink_by_border},
};

pub(crate) struct App {
    pub(crate) panes: Vec<Pane>,
    pub(crate) layout: Node,
    pub(crate) focused: usize,
    next_pane_id: usize,
    pub(crate) running: bool,
    pub(crate) reload_requested: bool,
    pub(crate) modal: Option<Modal>,
    theme_index: usize,
    pub(crate) theme_preview_index: usize,
    last_title_click: Option<(usize, Instant)>,
}

impl App {
    pub(crate) fn new(rows: u16, cols: u16) -> anyhow::Result<Self> {
        let content_rows = rows.saturating_sub(2).max(1);
        let content_cols = cols.saturating_sub(3).max(1);
        let persisted = load_persisted_layout();
        let layout = persisted
            .as_ref()
            .map(|state| state.layout.clone())
            .unwrap_or(Node::Leaf { pane_id: 0 });

        let mut pane_ids = Vec::new();
        layout.collect_leaf_ids(&mut pane_ids);
        if pane_ids.is_empty() {
            pane_ids.push(0);
        }

        let panes = pane_ids
            .iter()
            .copied()
            .map(|id| {
                let title = persisted
                    .as_ref()
                    .and_then(|state| state.titles.get(&id).cloned())
                    .unwrap_or_else(|| format!("Pane {}", id + 1));
                Pane::new(id, title, content_rows, content_cols)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let focused = persisted
            .as_ref()
            .map(|state| state.focused)
            .filter(|pane_id| layout.contains_pane_id(*pane_id))
            .unwrap_or_else(|| layout.first_leaf_id());
        let next_pane_id = layout.max_leaf_id().saturating_add(1);
        let theme_index = load_persisted_theme_index().unwrap_or(0);

        Ok(Self {
            panes,
            layout,
            focused,
            next_pane_id,
            running: true,
            reload_requested: false,
            modal: None,
            theme_index,
            theme_preview_index: theme_index,
            last_title_click: None,
        })
    }

    pub(crate) fn resize(&mut self, total_rows: u16, cols: u16) {
        let placements = self.pane_placements(Rect {
            x: 0,
            y: 0,
            width: cols,
            height: total_rows,
        });

        for placement in placements {
            if let Some(pane) = self.pane_mut(placement.pane_id) {
                let inner = shrink_by_border(placement.area);
                let content_cols = if inner.width > 1 {
                    inner.width - 1
                } else {
                    inner.width.max(1)
                };
                let content_rows = inner.height.max(1);
                pane.resize(content_rows, content_cols);
            }
        }
    }

    pub(crate) fn tick(&mut self) {
        for pane in &mut self.panes {
            pane.pump();
        }
    }

    pub(crate) fn persist_layout(&self) {
        let _ = save_persisted_layout(&self.layout, self.focused, &self.panes);
    }

    pub(crate) fn close_pane(&mut self) {
        if self.panes.len() <= 1 {
            self.running = false;
            return;
        }

        let focused = self.focused;
        let Some(pos) = self.panes.iter().position(|pane| pane.id == focused) else {
            return;
        };

        let Some(next_focus) = self.layout.delete_leaf(focused) else {
            return;
        };

        self.panes.remove(pos);
        self.focused = next_focus;
        self.persist_layout();
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent, size: Rect) -> anyhow::Result<()> {
        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::Help => {
                    if (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char(' '))
                        || key.code == KeyCode::Esc
                    {
                        self.modal = None;
                        return Ok(());
                    }

                    if matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T')) {
                        self.theme_preview_index = self.theme_index;
                        self.modal = Some(Modal::Theme);
                    } else {
                        self.modal = Some(Modal::Help);
                    }
                    return Ok(());
                }
                Modal::Theme => {
                    if (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char(' '))
                        || key.code == KeyCode::Esc
                    {
                        self.modal = Some(Modal::Help);
                        return Ok(());
                    }

                    match key.code {
                        KeyCode::Up | KeyCode::Left => {
                            self.theme_preview_index = if self.theme_preview_index == 0 {
                                THEMES.len() - 1
                            } else {
                                self.theme_preview_index - 1
                            };
                        }
                        KeyCode::Down | KeyCode::Right => {
                            self.theme_preview_index = (self.theme_preview_index + 1) % THEMES.len();
                        }
                        KeyCode::Enter => {
                            self.theme_index = self.theme_preview_index;
                            let _ = save_persisted_theme(self.theme());
                            self.modal = Some(Modal::Help);
                            return Ok(());
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(idx) = c.to_digit(10).map(|n| n as usize) {
                                if idx >= 1 && idx <= THEMES.len() {
                                    self.theme_preview_index = idx - 1;
                                }
                            }
                        }
                        _ => {}
                    }
                    self.modal = Some(Modal::Theme);
                    return Ok(());
                }
                Modal::Rename { pane_id, mut input } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Enter => {
                            if let Some(pane) = self.pane_mut(pane_id) {
                                pane.title = input;
                                self.persist_layout();
                            }
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            input.push(c);
                        }
                        _ => {}
                    }

                    self.modal = Some(Modal::Rename { pane_id, input });
                    return Ok(());
                }
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
            self.modal = Some(Modal::Help);
            return Ok(());
        }

        if is_close_pane_shortcut(&key) {
            self.close_pane();
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT)
        {
            if let Some(side) = arrow_key_to_split_side(key.code) {
                let focused = self.focused;
                let new_id = self.split_pane(focused, side)?;
                self.focused = new_id;
                return Ok(());
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(side) = arrow_key_to_split_side(key.code) {
                self.focus_adjacent(size, side);
                return Ok(());
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            self.running = false;
            self.reload_requested = true;
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            self.running = false;
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::SHIFT) {
            if let Some(pane) = self.focused_pane_mut() {
                match key.code {
                    KeyCode::PageUp => {
                        pane.page_up();
                        return Ok(());
                    }
                    KeyCode::PageDown => {
                        pane.page_down();
                        return Ok(());
                    }
                    KeyCode::Home => {
                        pane.scroll_top();
                        return Ok(());
                    }
                    KeyCode::End => {
                        pane.scroll_bottom();
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        let bytes = key_to_bytes(key);
        if !bytes.is_empty() {
            if let Some(pane) = self.focused_pane_mut() {
                pane.send(&bytes)?;
            }
        }

        Ok(())
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, size: Rect) -> anyhow::Result<()> {
        match self.modal {
            Some(Modal::Help) => {
                if contains(
                    help_close_button_area(help_modal_area(size)),
                    mouse.column,
                    mouse.row,
                ) && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                {
                    self.modal = None;
                }
                return Ok(());
            }
            Some(Modal::Theme) => {
                return Ok(());
            }
            Some(Modal::Rename { pane_id, .. }) => {
                let area = rename_modal_area(size);
                if contains(rename_close_button_area(area), mouse.column, mouse.row)
                    && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                {
                    self.modal = None;
                    self.focused = pane_id;
                    self.close_pane();
                }
                return Ok(());
            }
            None => {}
        }

        let placements = self.pane_placements(size);

        for placement in placements {
            if !contains(placement.area, mouse.column, mouse.row) {
                continue;
            }

            let Some(pane) = self.panes.iter().find(|pane| pane.id == placement.pane_id) else {
                continue;
            };

            self.focused = placement.pane_id;

            if placement.title_hit(&pane.title, self.focused == placement.pane_id, mouse.column, mouse.row)
                && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            {
                let now = Instant::now();
                if self
                    .last_title_click
                    .is_some_and(|(pane_id, last)| pane_id == placement.pane_id && now.duration_since(last) <= Duration::from_millis(500))
                {
                    self.last_title_click = None;
                    self.modal = Some(Modal::Rename {
                        pane_id: placement.pane_id,
                        input: pane.title.clone(),
                    });
                } else {
                    self.last_title_click = Some((placement.pane_id, now));
                }
                return Ok(());
            }

            if let Some(side) = placement.plus_hit(mouse.column, mouse.row) {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                    let new_id = self.split_pane(placement.pane_id, side)?;
                    self.focused = new_id;
                }
                return Ok(());
            }

            let inner = shrink_by_border(placement.area);
            if let Some(scrollbar_x) = inner.right().checked_sub(1) {
                if mouse.column == scrollbar_x {
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        let max_scroll = pane.scrollback_max;
                        if max_scroll > 0 {
                            let track_top = inner.top();
                            let track_bottom = inner.bottom().saturating_sub(1);
                            if track_bottom > track_top {
                                let clamped_row = mouse.row.clamp(track_top, track_bottom);
                                let track_len = (track_bottom - track_top).max(1) as usize;
                                let offset = (clamped_row - track_top) as usize;
                                let pos = ((offset * max_scroll) + track_len / 2) / track_len;
                                pane.scrollback = max_scroll.saturating_sub(pos.min(max_scroll));
                                pane.parser.set_scrollback(pane.scrollback);
                            }
                        }
                    }
                    return Ok(());
                }
            }

            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        pane.scroll_up();
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        pane.scroll_down();
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {}
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(pane) = self.pane_mut(placement.pane_id) {
                        if pane.rows > 0 {
                            if mouse.row <= placement.area.top() + 1 {
                                pane.page_up();
                            } else if mouse.row >= placement.area.bottom().saturating_sub(2) {
                                pane.page_down();
                            }
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        Ok(())
    }

    pub(crate) fn split_pane(&mut self, pane_id: usize, side: SplitSide) -> anyhow::Result<usize> {
        let new_id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);

        let title = format!("Pane {}", new_id + 1);
        self.panes.push(Pane::new(new_id, title, 1, 1)?);

        if self.layout.split_leaf(pane_id, side, new_id) {
            Ok(new_id)
        } else {
            self.next_pane_id = self.next_pane_id.saturating_sub(1);
            let _ = self.panes.pop();
            anyhow::bail!("pane not found")
        }
    }

    pub(crate) fn focus_adjacent(&mut self, size: Rect, side: SplitSide) {
        let placements = self.pane_placements(size);
        let Some(current_area) = placements
            .iter()
            .find(|placement| placement.pane_id == self.focused)
            .map(|placement| placement.area)
        else {
            return;
        };

        let mut candidates: Vec<_> = placements
            .into_iter()
            .filter(|placement| {
                placement.pane_id != self.focused
                    && placement_is_adjacent(current_area, placement.area, side)
            })
            .collect();

        candidates.sort_by_key(|placement| {
            std::cmp::Reverse(adjacent_overlap(current_area, placement.area, side))
        });

        if let Some(next) = candidates.first() {
            self.focused = next.pane_id;
        }
    }

    pub(crate) fn pane_placements(&self, size: Rect) -> Vec<Placement> {
        let mut placements = Vec::new();
        self.layout.collect(
            size,
            ExposedSides {
                top: true,
                bottom: true,
                left: true,
                right: true,
            },
            &mut placements,
        );
        placements
    }

    pub(crate) fn theme(&self) -> Theme {
        THEMES[self.theme_index]
    }

    pub(crate) fn preview_theme(&self) -> Theme {
        THEMES[self.theme_preview_index]
    }

    pub(crate) fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        self.pane_mut(self.focused)
    }

    pub(crate) fn pane_mut(&mut self, pane_id: usize) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }
}
