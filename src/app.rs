use std::{
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Direction, Rect};

use crate::{
    layout::{
        adjacent_overlap, load_persisted_layout, pane_inner_area, placement_is_adjacent,
        save_persisted_layout, DebugContainer, DebugPlacement, ExposedSides, Node, Placement,
        ResizeBoundary, SplitSide,
    },
    pane::Pane,
    theme::{load_persisted_theme_index, save_persisted_theme, Theme, THEMES},
    ui::{
        close_confirm_cancel_button_area, close_confirm_confirm_button_area,
        close_confirm_modal_area, default_agent_button_area, default_agent_dropdown_area,
        help_close_button_area, help_debug_toggle_button_area, help_modal_area,
        panel_settings_agent_list_area, panel_settings_cancel_button_area,
        panel_settings_close_button_area, panel_settings_confirm_button_area,
        panel_settings_modal_area, panel_settings_modal_inner, panel_settings_name_input_area,
        settings_button_area, Modal, PanelSettingsFocus, AGENT_PRESETS,
    },
    utils::{arrow_key_to_split_side, contains, key_to_bytes},
};

pub(crate) struct App {
    pub(crate) panes: Vec<Pane>,
    pub(crate) layout: Node,
    pub(crate) focused: usize,
    maximized_pane: Option<usize>,
    next_pane_id: usize,
    pub(crate) running: bool,
    pub(crate) reload_requested: bool,
    pub(crate) modal: Option<Modal>,
    drag_resize: Option<DragResize>,
    theme_index: usize,
    pub(crate) default_agent_index: usize,
    pub(crate) theme_preview_index: usize,
    last_title_click: Option<(usize, Instant)>,
    debug_container_boxes: bool,
    mouse_capture_enabled: bool,
}

#[derive(Clone)]
struct DragResize {
    pane_a: usize,
    pane_b: usize,
    direction: Direction,
    last_coord: u16,
    pane_ids: Vec<usize>,
}

struct ResizeTarget {
    pane_a: usize,
    pane_b: usize,
    direction: Direction,
    pane_ids: Vec<usize>,
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

        let default_agent_index = persisted
            .as_ref()
            .map(|state| state.default_agent_index)
            .unwrap_or(1)
            .min(AGENT_PRESETS.len().saturating_sub(1));
        let panes = pane_ids
            .iter()
            .copied()
            .map(|id| {
                let title = persisted
                    .as_ref()
                    .and_then(|state| state.titles.get(&id).cloned())
                    .unwrap_or_else(|| format!("Pane {}", id + 1));
                let command = persisted
                    .as_ref()
                    .and_then(|state| state.commands.get(&id).cloned())
                    .unwrap_or_else(|| AGENT_PRESETS[default_agent_index].command.to_string());
                let command = normalize_stored_agent_command(command);
                let resume_command = persisted
                    .as_ref()
                    .and_then(|state| state.resume_commands.get(&id).cloned());
                Pane::new(
                    id,
                    title,
                    command,
                    resume_command,
                    content_rows,
                    content_cols,
                )
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
            maximized_pane: None,
            next_pane_id,
            running: true,
            reload_requested: false,
            modal: None,
            drag_resize: None,
            theme_index,
            default_agent_index,
            theme_preview_index: theme_index,
            last_title_click: None,
            debug_container_boxes: parse_debug_flag("SPLIT_TUI_DEBUG_CONTAINERS"),
            mouse_capture_enabled: true,
        })
    }

    pub(crate) fn content_area(size: Rect) -> Rect {
        Rect {
            x: size.x,
            y: size.y.saturating_add(1),
            width: size.width,
            height: size.height.saturating_sub(1),
        }
    }

    pub(crate) fn resize(&mut self, total_rows: u16, cols: u16) {
        let placements = self.pane_placements(Self::content_area(Rect {
            x: 0,
            y: 0,
            width: cols,
            height: total_rows,
        }));

        for placement in placements {
            if let Some(pane) = self.pane_mut(placement.pane_id) {
                let inner = pane_inner_area(placement.area, placement.exposed);
                let content_cols = inner.width.saturating_sub(1).max(1);
                let content_rows = inner.height.max(1);
                pane.resize(content_rows, content_cols);
            }
        }
    }

    /// Drain PTY output for all panes. Returns true if any pane processed new
    /// bytes (i.e. the screen may need to be redrawn). Panes whose program
    /// has exited are automatically respawned as a login shell so the user
    /// can keep using the slot instead of staring at a frozen view.
    pub(crate) fn tick(&mut self) -> bool {
        let mut any = false;
        for pane in &mut self.panes {
            if pane.pump() {
                any = true;
            }
            if pane.exited && !pane.relaunch_failed {
                match pane.relaunch_as_shell() {
                    Ok(()) => any = true,
                    Err(_) => pane.relaunch_failed = true,
                }
            }
        }
        any
    }

    /// Graceful shutdown: ask every running agent to exit (SIGTERM), drain
    /// their output for up to `max_wait` so resume hints printed during exit
    /// land in the parser, then take a final best-effort snapshot. Called
    /// from the run loop after the user requests quit / reload so the
    /// next launch can resume each pane where it left off.
    pub(crate) fn shutdown_panes(&mut self, max_wait: Duration) {
        // 1. Politely ask each not-yet-exited child to exit. Most agents have
        //    a SIGTERM handler that flushes session state and prints a
        //    resume hint before terminating.
        for pane in &self.panes {
            if !pane.exited {
                pane.request_exit();
            }
        }

        // 2. Drain output until every pane has reported exit or we run out
        //    of patience. We poll rather than block because each pane's PTY
        //    reader runs on its own thread and may produce output at any
        //    time during the shutdown window.
        let deadline = Instant::now() + max_wait;
        loop {
            let mut all_done = true;
            for pane in &mut self.panes {
                pane.pump();
                if !pane.exited {
                    all_done = false;
                }
            }
            if all_done || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        // 3. Final sweep: some agents print their resume line and then hang
        //    (or take longer than `max_wait` to actually close fds). Scrape
        //    the on-screen contents one more time so we still capture the
        //    hint even if the child hasn't fully disconnected yet.
        for pane in &mut self.panes {
            if pane.resume_command.is_none() {
                pane.try_capture_resume_command();
            }
        }
    }

    pub(crate) fn persist_layout(&self) {
        let _ = save_persisted_layout(
            &self.layout,
            self.focused,
            self.default_agent_index,
            &self.panes,
        );
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
        self.focus_pane(next_focus);
        self.persist_layout();
    }

    pub(crate) fn apply_panel_settings(
        &mut self,
        pane_id: usize,
        name: String,
        agent_index: usize,
    ) -> anyhow::Result<()> {
        let Some(pos) = self.panes.iter().position(|pane| pane.id == pane_id) else {
            return Ok(());
        };

        let (rows, cols) = {
            let pane = &self.panes[pos];
            (pane.rows, pane.cols)
        };
        let command = AGENT_PRESETS
            .get(agent_index)
            .map(|p| p.command)
            .unwrap_or(AGENT_PRESETS[0].command)
            .to_string();

        // Changing the agent of a pane discards any prior resume hint.
        self.panes[pos] = Pane::new(pane_id, name, command, None, rows, cols)?;
        self.focus_pane(pane_id);
        self.persist_layout();
        Ok(())
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent, size: Rect) -> anyhow::Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('m') | KeyCode::Char('M'))
        {
            self.mouse_capture_enabled = !self.mouse_capture_enabled;
            self.drag_resize = None;
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            if let Some(pane) = self.focused_pane_mut() {
                pane.send(&[0x03])?;
            }
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W'))
        {
            self.modal = Some(Modal::CloseConfirm {
                pane_id: self.focused,
            });
            return Ok(());
        }

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

                    if matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D')) {
                        self.debug_container_boxes = !self.debug_container_boxes;
                        self.modal = Some(Modal::Help);
                    } else if matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T')) {
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
                            self.theme_preview_index =
                                (self.theme_preview_index + 1) % THEMES.len();
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
                Modal::DefaultAgent { mut agent_index } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Up | KeyCode::Left => {
                            agent_index = if agent_index == 0 {
                                AGENT_PRESETS.len() - 1
                            } else {
                                agent_index - 1
                            };
                        }
                        KeyCode::Down | KeyCode::Right => {
                            agent_index = (agent_index + 1) % AGENT_PRESETS.len();
                        }
                        KeyCode::Enter => {
                            self.default_agent_index = agent_index;
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            if let Some(idx) = c.to_digit(10).map(|n| n as usize) {
                                if idx >= 1 && idx <= AGENT_PRESETS.len() {
                                    agent_index = idx - 1;
                                }
                            }
                        }
                        _ => {}
                    }

                    self.modal = Some(Modal::DefaultAgent { agent_index });
                    return Ok(());
                }
                Modal::PanelSettings {
                    pane_id,
                    mut name,
                    mut agent_index,
                    mut focus,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Tab => {
                            focus = focus.next();
                        }
                        KeyCode::BackTab => {
                            focus = focus.prev();
                        }
                        KeyCode::Enter => {
                            self.apply_panel_settings(pane_id, name, agent_index)?;
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Backspace if focus == PanelSettingsFocus::Name => {
                            name.pop();
                        }
                        KeyCode::Char(c)
                            if focus == PanelSettingsFocus::Name
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            name.push(c);
                        }
                        KeyCode::Left | KeyCode::Up if focus == PanelSettingsFocus::Agent => {
                            agent_index = if agent_index == 0 {
                                AGENT_PRESETS.len() - 1
                            } else {
                                agent_index - 1
                            };
                        }
                        KeyCode::Right | KeyCode::Down if focus == PanelSettingsFocus::Agent => {
                            agent_index = (agent_index + 1) % AGENT_PRESETS.len();
                        }
                        _ => {}
                    }

                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        agent_index,
                        focus,
                    });
                    return Ok(());
                }
                Modal::CloseConfirm { pane_id } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.modal = None;
                        }
                        KeyCode::Enter => {
                            self.modal = None;
                            self.focus_pane(pane_id);
                            self.close_pane();
                        }
                        _ => {
                            self.modal = Some(Modal::CloseConfirm { pane_id });
                        }
                    }
                    return Ok(());
                }
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
            self.modal = Some(Modal::Help);
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            match key.code {
                KeyCode::Left => {
                    self.resize_focused_edge(size, SplitSide::Left, 10)?;
                    return Ok(());
                }
                KeyCode::Right => {
                    self.resize_focused_edge(size, SplitSide::Right, 10)?;
                    return Ok(());
                }
                KeyCode::Up => {
                    self.resize_focused_edge(size, SplitSide::Top, 10)?;
                    return Ok(());
                }
                KeyCode::Down => {
                    self.resize_focused_edge(size, SplitSide::Bottom, 10)?;
                    return Ok(());
                }
                KeyCode::Char('k' | 'K') => {
                    self.resize_focused_edge(size, SplitSide::Top, 10)?;
                    return Ok(());
                }
                KeyCode::Char('j' | 'J') => {
                    self.resize_focused_edge(size, SplitSide::Bottom, 10)?;
                    return Ok(());
                }
                KeyCode::Char('a' | 'A') => {
                    let focused = self.focused;
                    let new_id = self.split_pane(focused, SplitSide::Right, size)?;
                    self.focus_pane(new_id);
                    return Ok(());
                }
                KeyCode::Char('b' | 'B') => {
                    let focused = self.focused;
                    let new_id = self.split_pane(focused, SplitSide::Bottom, size)?;
                    self.focus_pane(new_id);
                    return Ok(());
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT)
        {
            if let Some(side) = arrow_key_to_split_side(key.code) {
                let focused = self.focused;
                let new_id = self.split_pane(focused, side, size)?;
                self.focus_pane(new_id);
                return Ok(());
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            match key.code {
                KeyCode::PageUp => self.focus_prev_pane(),
                KeyCode::PageDown => self.focus_next_pane(),
                _ => {}
            }
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(side) = arrow_key_to_split_side(key.code) {
                self.focus_adjacent(size, side);
                return Ok(());
            }
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

    pub(crate) fn handle_paste(&mut self, text: String) -> anyhow::Result<()> {
        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::PanelSettings {
                    pane_id,
                    mut name,
                    agent_index,
                    focus,
                } if focus == PanelSettingsFocus::Name => {
                    name.push_str(&text);
                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        agent_index,
                        focus,
                    });
                }
                other => {
                    self.modal = Some(other);
                }
            }
            return Ok(());
        }

        if let Some(pane) = self.focused_pane_mut() {
            pane.send_paste(&text)?;
        }

        Ok(())
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, size: Rect) -> anyhow::Result<()> {
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.drag_resize = None;
        }

        if let Some(drag) = self.drag_resize.clone() {
            if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
                let coord = match drag.direction {
                    Direction::Vertical => mouse.row,
                    Direction::Horizontal => mouse.column,
                };
                let delta = i32::from(coord) - i32::from(drag.last_coord);
                if delta != 0 {
                    let side = match (drag.direction, delta > 0) {
                        (Direction::Horizontal, true) => SplitSide::Right,
                        (Direction::Horizontal, false) => SplitSide::Left,
                        (Direction::Vertical, true) => SplitSide::Bottom,
                        (Direction::Vertical, false) => SplitSide::Top,
                    };
                    self.resize_between_panes(
                        size,
                        drag.pane_a,
                        drag.pane_b,
                        side,
                        delta.unsigned_abs() as u16,
                    )?;
                    self.drag_resize = Some(DragResize {
                        pane_a: drag.pane_a,
                        pane_b: drag.pane_b,
                        direction: drag.direction,
                        last_coord: coord,
                        pane_ids: drag.pane_ids,
                    });
                }
                return Ok(());
            }
        }

        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::Help => {
                    let help_area = help_modal_area(size);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(help_close_button_area(help_area), mouse.column, mouse.row) {
                            self.modal = None;
                            return Ok(());
                        }
                        if contains(
                            help_debug_toggle_button_area(help_area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.debug_container_boxes = !self.debug_container_boxes;
                        }
                    }
                    self.modal = Some(Modal::Help);
                    return Ok(());
                }
                Modal::Theme => {
                    self.modal = Some(Modal::Theme);
                    return Ok(());
                }
                Modal::DefaultAgent { agent_index } => {
                    let area = default_agent_dropdown_area(size);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(area, mouse.column, mouse.row) {
                            let inner = Rect {
                                x: area.x + 1,
                                y: area.y + 1,
                                width: area.width.saturating_sub(2),
                                height: area.height.saturating_sub(2),
                            };
                            let selected = mouse.row.saturating_sub(inner.y) as usize;
                            if selected < AGENT_PRESETS.len() {
                                self.default_agent_index = selected;
                                self.modal = None;
                                return Ok(());
                            }
                        }
                    }

                    self.modal = Some(Modal::DefaultAgent { agent_index });
                    return Ok(());
                }
                Modal::PanelSettings {
                    pane_id,
                    name,
                    mut agent_index,
                    mut focus,
                } => {
                    let area = panel_settings_modal_area(size);
                    let inner = panel_settings_modal_inner(area);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(
                            panel_settings_close_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) || contains(
                            panel_settings_cancel_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                            return Ok(());
                        }

                        if contains(
                            panel_settings_confirm_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.apply_panel_settings(pane_id, name, agent_index)?;
                            self.modal = None;
                            return Ok(());
                        }

                        let name_area = panel_settings_name_input_area(inner);
                        if contains(name_area, mouse.column, mouse.row) {
                            focus = PanelSettingsFocus::Name;
                        }

                        let agent_area = panel_settings_agent_list_area(inner);
                        if contains(agent_area, mouse.column, mouse.row) {
                            let selected = mouse.row.saturating_sub(agent_area.y + 1) as usize;
                            if selected < AGENT_PRESETS.len() {
                                agent_index = selected;
                                focus = PanelSettingsFocus::Agent;
                            }
                        }
                    }

                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        agent_index,
                        focus,
                    });
                    return Ok(());
                }
                Modal::CloseConfirm { pane_id } => {
                    let area = close_confirm_modal_area(size);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(
                            close_confirm_cancel_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                        } else if contains(
                            close_confirm_confirm_button_area(area),
                            mouse.column,
                            mouse.row,
                        ) {
                            self.modal = None;
                            self.focus_pane(pane_id);
                            self.close_pane();
                        } else {
                            self.modal = Some(Modal::CloseConfirm { pane_id });
                        }
                    } else {
                        self.modal = Some(Modal::CloseConfirm { pane_id });
                    }
                    return Ok(());
                }
            }
        }

        if contains(settings_button_area(size), mouse.column, mouse.row)
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.modal = Some(Modal::Help);
            return Ok(());
        }

        if contains(
            default_agent_button_area(size, AGENT_PRESETS[self.default_agent_index].label),
            mouse.column,
            mouse.row,
        ) && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            let agent_index = self.default_agent_index;
            self.modal = Some(Modal::DefaultAgent { agent_index });
            return Ok(());
        }

        let clicked = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        let Some(placement) = self.placement_at(size, mouse.column, mouse.row) else {
            return Ok(());
        };

        let Some(pane_title) = self
            .panes
            .iter()
            .find(|pane| pane.id == placement.pane_id)
            .map(|pane| pane.title.clone())
        else {
            return Ok(());
        };

        let was_focused = self.focused == placement.pane_id;

        if clicked {
            // The common case is a plain click in terminal content. Focus it now
            // and skip the expensive divider/boundary walk entirely.
            let inner = pane_inner_area(placement.area, placement.exposed);
            if contains(inner, mouse.column, mouse.row) {
                self.focus_pane(placement.pane_id);
                return Ok(());
            }

            let chrome_hit = placement.title_hit(&pane_title, was_focused, mouse.column, mouse.row)
                || placement.maximize_hit(mouse.column, mouse.row)
                || placement.close_hit(mouse.column, mouse.row);
            if !chrome_hit {
                if let Some(target) = self.resize_target_at(size, mouse.column, mouse.row) {
                    self.focus_pane(target.pane_a);
                    self.drag_resize = Some(DragResize {
                        pane_a: target.pane_a,
                        pane_b: target.pane_b,
                        direction: target.direction,
                        last_coord: match target.direction {
                            Direction::Vertical => mouse.row,
                            Direction::Horizontal => mouse.column,
                        },
                        pane_ids: target.pane_ids,
                    });
                    return Ok(());
                }
            }
        }

        if clicked && placement.maximize_hit(mouse.column, mouse.row) {
            self.focus_pane(placement.pane_id);
            self.toggle_maximize();
            return Ok(());
        }

        if clicked && placement.close_hit(mouse.column, mouse.row) {
            self.focus_pane(placement.pane_id);
            self.modal = Some(Modal::CloseConfirm {
                pane_id: placement.pane_id,
            });
            return Ok(());
        }

        if clicked && placement.title_hit(&pane_title, was_focused, mouse.column, mouse.row) {
            self.focus_pane(placement.pane_id);
            let now = Instant::now();
            if self.last_title_click.is_some_and(|(pane_id, last)| {
                pane_id == placement.pane_id
                    && now.duration_since(last) <= Duration::from_millis(500)
            }) {
                self.last_title_click = None;
                let agent_index = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == placement.pane_id)
                    .map(|pane| agent_index_for_command(&pane.command))
                    .unwrap_or(0);
                self.modal = Some(Modal::PanelSettings {
                    pane_id: placement.pane_id,
                    name: pane_title.clone(),
                    agent_index,
                    focus: PanelSettingsFocus::Name,
                });
            } else {
                self.last_title_click = Some((placement.pane_id, now));
            }
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if let Some(pane) = self.pane_mut(placement.pane_id) {
                    let inner = pane_inner_area(placement.area, placement.exposed);
                    let x = mouse.column.saturating_sub(inner.x);
                    let y = mouse.row.saturating_sub(inner.y);
                    if pane.scrollback_max > 0 || !pane.send_mouse_wheel(true, x, y)? {
                        pane.scroll_up();
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(pane) = self.pane_mut(placement.pane_id) {
                    let inner = pane_inner_area(placement.area, placement.exposed);
                    let x = mouse.column.saturating_sub(inner.x);
                    let y = mouse.row.saturating_sub(inner.y);
                    if pane.scrollback_max > 0 || !pane.send_mouse_wheel(false, x, y)? {
                        pane.scroll_down();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus_pane(placement.pane_id);
            }
            _ => {}
        }
        return Ok(());
    }

    pub(crate) fn split_pane(
        &mut self,
        pane_id: usize,
        side: SplitSide,
        terminal_size: Rect,
    ) -> anyhow::Result<usize> {
        let new_id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);

        let title = format!("Pane {}", new_id + 1);
        self.panes.push(Pane::new(
            new_id,
            title,
            AGENT_PRESETS[self.default_agent_index].command,
            None,
            1,
            1,
        )?);

        if self.layout.split_leaf(pane_id, side, new_id) {
            self.resize(terminal_size.height, terminal_size.width);
            Ok(new_id)
        } else {
            self.next_pane_id = self.next_pane_id.saturating_sub(1);
            let _ = self.panes.pop();
            anyhow::bail!("pane not found")
        }
    }

    pub(crate) fn focus_adjacent(&mut self, size: Rect, side: SplitSide) {
        let placements = self.pane_placements(Self::content_area(size));
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
            self.focus_pane(next.pane_id);
        }
    }

    fn focus_next_pane(&mut self) {
        let mut ids = Vec::new();
        self.layout.collect_leaf_ids(&mut ids);
        if ids.len() <= 1 {
            return;
        }
        let pos = ids.iter().position(|&id| id == self.focused).unwrap_or(0);
        let next = ids[(pos + 1) % ids.len()];
        self.focus_pane(next);
    }

    fn focus_prev_pane(&mut self) {
        let mut ids = Vec::new();
        self.layout.collect_leaf_ids(&mut ids);
        if ids.len() <= 1 {
            return;
        }
        let pos = ids.iter().position(|&id| id == self.focused).unwrap_or(0);
        let prev = ids[(pos + ids.len() - 1) % ids.len()];
        self.focus_pane(prev);
    }

    pub(crate) fn resize_focused_edge(
        &mut self,
        size: Rect,
        side: SplitSide,
        amount: u16,
    ) -> anyhow::Result<()> {
        let focused = self.focused;
        let Some(exposed) = self
            .pane_placements(Self::content_area(size))
            .into_iter()
            .find(|placement| placement.pane_id == focused)
            .map(|placement| placement.exposed)
        else {
            return Ok(());
        };

        let content_size = Self::content_area(size);
        let mut ok = self
            .layout
            .resize_leaf_edge(focused, side, amount, content_size, exposed);

        if !ok && matches!(side, SplitSide::Top | SplitSide::Bottom) {
            if self.panes.len() > 1 && !self.layout.has_vertical_split() {
                self.insert_root_vertical_split(size)?;
                ok = self
                    .layout
                    .resize_leaf_edge(focused, side, amount, content_size, exposed);
            }
        }

        if ok {
            self.persist_layout();
            self.resize(size.height, size.width);
        }
        Ok(())
    }

    /// Purely horizontal layouts only apportion width. Height resize needs at least one
    /// vertical split; we add a root row (current grid on top, new pane below) on first ↑/↓ resize.
    fn insert_root_vertical_split(&mut self, terminal: Rect) -> anyhow::Result<()> {
        let new_id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        let title = format!("Pane {}", new_id + 1);
        self.panes.push(Pane::new(
            new_id,
            title,
            AGENT_PRESETS[self.default_agent_index].command,
            None,
            1,
            1,
        )?);

        let upper = std::mem::replace(&mut self.layout, Node::Leaf { pane_id: new_id });
        self.layout = Node::Split {
            direction: Direction::Vertical,
            ratio: 50,
            first: Box::new(upper),
            second: Box::new(Node::Leaf { pane_id: new_id }),
        };

        self.resize(terminal.height, terminal.width);
        Ok(())
    }

    pub(crate) fn pane_placements(&self, size: Rect) -> Vec<Placement> {
        if let Some(pane_id) = self.maximized_pane {
            return vec![Placement {
                pane_id,
                area: size,
                exposed: ExposedSides {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
            }];
        }

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

    fn placement_at(&self, size: Rect, x: u16, y: u16) -> Option<Placement> {
        let content = Self::content_area(size);
        if let Some(pane_id) = self.maximized_pane {
            return contains(content, x, y).then_some(Placement {
                pane_id,
                area: content,
                exposed: ExposedSides {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
            });
        }

        if self.debug_container_boxes {
            let (_, placements) = self.debug_layout_areas(content);
            return placements
                .into_iter()
                .find(|placement| contains(placement.pane_area, x, y))
                .map(|placement| Placement {
                    pane_id: placement.pane_id,
                    area: placement.pane_area,
                    exposed: ExposedSides {
                        top: true,
                        bottom: true,
                        left: true,
                        right: true,
                    },
                });
        }

        self.layout.placement_at(
            content,
            ExposedSides {
                top: true,
                bottom: true,
                left: true,
                right: true,
            },
            x,
            y,
        )
    }

    pub(crate) fn debug_layout_areas(
        &self,
        size: Rect,
    ) -> (Vec<DebugContainer>, Vec<DebugPlacement>) {
        if self.maximized_pane.is_some() {
            return (
                Vec::new(),
                self.pane_placements(size)
                    .into_iter()
                    .map(|placement| DebugPlacement {
                        pane_id: placement.pane_id,
                        container_area: placement.area,
                        pane_area: placement.area,
                    })
                    .collect(),
            );
        }

        let mut containers = Vec::new();
        let mut placements = Vec::new();
        self.layout
            .collect_debug_areas(size, &mut containers, &mut placements);
        (containers, placements)
    }

    pub(crate) fn debug_container_boxes(&self) -> bool {
        self.debug_container_boxes
    }

    pub(crate) fn mouse_capture_enabled(&self) -> bool {
        self.mouse_capture_enabled
    }

    pub(crate) fn resize_preview_pane_ids(&self) -> Option<&[usize]> {
        self.drag_resize
            .as_ref()
            .map(|drag| drag.pane_ids.as_slice())
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

    fn focus_pane(&mut self, pane_id: usize) {
        self.focused = pane_id;
        if self.maximized_pane.is_some() {
            self.maximized_pane = Some(pane_id);
        }
    }

    pub(crate) fn toggle_maximize(&mut self) {
        if self.maximized_pane.is_some() {
            self.maximized_pane = None;
        } else {
            self.maximized_pane = Some(self.focused);
        }
    }

    pub(crate) fn pane_mut(&mut self, pane_id: usize) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    fn resize_target_at(&self, size: Rect, x: u16, y: u16) -> Option<ResizeTarget> {
        if self.maximized_pane.is_some() {
            return None;
        }

        let mut boundaries = Vec::new();
        if self.debug_container_boxes {
            self.layout.collect_debug_resize_boundaries(
                Self::content_area(size),
                0,
                &mut boundaries,
            );
        } else {
            self.layout
                .collect_resize_boundaries(Self::content_area(size), 0, &mut boundaries);
        }

        let boundary = boundaries
            .into_iter()
            .filter(|boundary| resize_boundary_hit(boundary, x, y))
            .min_by_key(|boundary| boundary.depth)?;

        let pane_a = *boundary.first_pane_ids.first()?;
        let pane_b = *boundary.second_pane_ids.first()?;
        let mut pane_ids = boundary.first_pane_ids;
        pane_ids.extend(boundary.second_pane_ids);

        Some(ResizeTarget {
            pane_a,
            pane_b,
            direction: boundary.direction,
            pane_ids,
        })
    }

    fn resize_between_panes(
        &mut self,
        size: Rect,
        pane_a: usize,
        pane_b: usize,
        side: SplitSide,
        amount: u16,
    ) -> anyhow::Result<()> {
        let content_size = Self::content_area(size);
        let ok = if self.debug_container_boxes {
            self.layout
                .resize_between_debug(pane_a, pane_b, side, amount, content_size)
        } else {
            self.layout
                .resize_between(pane_a, pane_b, side, amount, content_size)
        };

        if ok {
            self.persist_layout();
            self.resize(size.height, size.width);
        }
        Ok(())
    }
}

fn parse_debug_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn resize_boundary_hit(boundary: &ResizeBoundary, x: u16, y: u16) -> bool {
    if let Some(divider_area) = boundary.divider_area {
        if contains(divider_area, x, y) {
            return true;
        }

        // Adjacent panes now draw independent edge cells. Treat either edge as
        // the same divider so dragging/clicking both container edges resizes the
        // pair, not just the second pane's leading edge.
        match boundary.direction {
            Direction::Horizontal => {
                let first_edge_x = boundary.first_area.right().saturating_sub(1);
                return x == first_edge_x
                    && ranges_overlap_at(
                        y,
                        boundary.first_area.y,
                        boundary.first_area.bottom(),
                        boundary.second_area.y,
                        boundary.second_area.bottom(),
                    );
            }
            Direction::Vertical => {
                let first_edge_y = boundary.first_area.bottom().saturating_sub(1);
                return y == first_edge_y
                    && ranges_overlap_at(
                        x,
                        boundary.first_area.x,
                        boundary.first_area.right(),
                        boundary.second_area.x,
                        boundary.second_area.right(),
                    );
            }
        }
    }

    match boundary.direction {
        Direction::Horizontal => {
            let boundary_x = boundary.second_area.x;
            x == boundary_x
                && ranges_overlap_at(
                    y,
                    boundary.first_area.y,
                    boundary.first_area.bottom(),
                    boundary.second_area.y,
                    boundary.second_area.bottom(),
                )
        }
        Direction::Vertical => {
            let boundary_y = boundary.second_area.y;
            y == boundary_y
                && ranges_overlap_at(
                    x,
                    boundary.first_area.x,
                    boundary.first_area.right(),
                    boundary.second_area.x,
                    boundary.second_area.right(),
                )
        }
    }
}

fn ranges_overlap_at(value: u16, start_a: u16, end_a: u16, start_b: u16, end_b: u16) -> bool {
    let start = start_a.max(start_b);
    let end = end_a.min(end_b);
    value >= start && value < end
}

fn normalize_stored_agent_command(command: String) -> String {
    match command.as_str() {
        "cursor-agent" => "agent".to_string(),
        "opencode-agent" => "opencode".to_string(),
        _ => command,
    }
}

fn agent_index_for_command(command: &str) -> usize {
    let normalized = match command {
        "cursor-agent" => "agent",
        "opencode-agent" => "opencode",
        _ => command,
    };
    AGENT_PRESETS
        .iter()
        .position(|preset| preset.command == normalized)
        .unwrap_or(1)
}
