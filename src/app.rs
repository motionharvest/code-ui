use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Direction, Rect};

use crate::{
    layout::{
        adjacent_overlap, load_persisted_layout, pane_inner_area, placement_is_adjacent,
        save_persisted_layout, DebugContainer, DebugPlacement, ExposedSides, Node,
        PersistedWorkspace, Placement, ResizeBoundary, SplitSide,
    },
    pane::{Pane, PaneMouseEventKind, PaneSelection},
    theme::{load_persisted_theme_index, save_persisted_theme, Theme, THEMES},
    ui::{
        default_agent_index, help_close_button_area,
        help_debug_toggle_button_area, help_modal_area, new_pane_picker_list_area,
        new_pane_picker_modal_area, new_pane_picker_name_input_area, pane_chrome_title_label,
        panel_settings_agent_list_area, panel_settings_cancel_button_area,
        panel_settings_close_button_area, panel_settings_confirm_button_area,
        panel_settings_modal_area, panel_settings_modal_inner, panel_settings_name_input_area,
        compute_top_bar_layout, workspace_add_button_hit, workspace_commander_input_hit,
        workspace_hit_index, workspace_menu_hit_index, workspace_settings_action_hit_index,
        workspace_settings_modal_area, workspace_settings_name_input_area, Modal,
        PanelSettingsFocus, AGENT_PRESETS, COMMANDER_COMMAND, TOP_CHROME_ROWS,
    },
    utils::{arrow_key_to_split_side, contains, key_to_bytes, LOGIN_SHELL_SENTINEL},
};

/// Max gap between two Ctrl+Q presses before quit is cancelled.
const QUIT_CONFIRM_WINDOW: Duration = Duration::from_millis(600);

pub(crate) struct App {
    pub(crate) panes: Vec<Pane>,
    workspaces: Vec<WorkspaceState>,
    active_workspace: usize,
    pub(crate) layout: Node,
    pub(crate) focused: usize,
    maximized_pane: Option<usize>,
    next_pane_id: usize,
    pub(crate) running: bool,
    pub(crate) reload_requested: bool,
    pub(crate) modal: Option<Modal>,
    drag_resize: Option<DragResize>,
    drag_swap: Option<DragPaneSwap>,
    drag_pane_mouse: Option<DragPaneMouse>,
    text_selection: Option<TextSelection>,
    theme_index: usize,
    pub(crate) default_agent_index: usize,
    pub(crate) theme_preview_index: usize,
    debug_container_boxes: bool,
    mouse_capture_enabled: bool,
    commander_focused: bool,
    sidebar_workspace_focused: Option<usize>,
    sidebar_add_button_focused: bool,
    commander: CommanderState,
    agent_handoffs: HashMap<usize, AgentHandoff>,
    commander_palette_video: CommanderPaletteVideoState,
    tts: TtsState,
    last_terminal_size: Rect,
    hit_test_cache: Option<HitTestCache>,
    last_quit_key_press: Option<Instant>,
}

#[derive(Clone)]
struct WorkspaceState {
    name: String,
    layout: Node,
    focused: usize,
    maximized_pane: Option<usize>,
}

#[derive(Clone)]
struct DragResize {
    pane_a: usize,
    pane_b: usize,
    direction: Direction,
    last_coord: u16,
    pane_ids: Vec<usize>,
}

#[derive(Clone)]
struct DragPaneSwap {
    source_pane_id: usize,
    hovered_pane_id: Option<usize>,
    moved: bool,
}

#[derive(Clone, Copy)]
struct DragPaneMouse {
    pane_id: usize,
    button: MouseButton,
}

#[derive(Clone, Copy)]
struct TextSelection {
    pane_id: usize,
    start: (u16, u16),
    end: (u16, u16),
    active: bool,
}

struct ResizeTarget {
    pane_a: usize,
    pane_b: usize,
    direction: Direction,
    pane_ids: Vec<usize>,
}

struct HitTestCache {
    content: Rect,
    placements: Vec<Placement>,
    row_candidates: Vec<Vec<usize>>,
    resize_boundaries: Vec<ResizeBoundary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommanderPhase {
    Discussing,
    AwaitingApproval,
    Executing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CommanderWorkspacePlan {
    UseCurrent,
    CreateNew,
    SwitchTo(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommanderPaneAssignment {
    agent: String,
    pane_name: String,
    task: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommanderPendingPlan {
    summary: String,
    objective: String,
    workspace: CommanderWorkspacePlan,
    workspace_name: Option<String>,
    panes: Vec<CommanderPaneAssignment>,
}

struct CommanderState {
    input: String,
    cursor: usize,
    busy: bool,
    phase: CommanderPhase,
    pending_plan: Option<CommanderPendingPlan>,
    history: Vec<String>,
    /// Wrapped chat lines scrolled up from the bottom of the transcript.
    chat_offset_from_bottom: usize,
    chat_pinned_to_bottom: bool,
    /// Last rendered chat viewport height (wrapped display lines).
    chat_viewport_lines: u16,
    /// Total wrapped chat lines from the last render pass.
    chat_total_lines: usize,
    rx: Option<Receiver<CommanderWorkerResult>>,
}

struct CommanderWorkerResult {
    steps: Vec<CommanderExecutionStep>,
    reply_text: Option<String>,
    speech_text: Option<String>,
    target_name: Option<String>,
    payload: Option<String>,
    submit_payload: bool,
    create_requests: Vec<(String, usize)>,
    rename_requests: Vec<(String, String)>,
    close_requests: Vec<String>,
    workspace_switch: Option<String>,
    workspace_create: Option<Option<String>>,
    proposed_plan: Option<CommanderPendingPlan>,
    /// When false, Commander only updates the conversation (no pane/workspace actions).
    execution_allowed: bool,
}

#[derive(Clone, Default)]
struct CommanderExecutionStep {
    save_as: Option<String>,
    reply_text: Option<String>,
    speech_text: Option<String>,
    target_name: Option<String>,
    payload: Option<String>,
    submit_payload: bool,
    create_requests: Vec<(String, usize)>,
    rename_requests: Vec<(String, String)>,
    close_requests: Vec<String>,
    workspace_switch: Option<String>,
    workspace_create: Option<Option<String>>,
}

#[derive(Default)]
struct CommanderStepRefs {
    named: HashMap<String, Vec<usize>>,
    last_created_ids: Vec<usize>,
}

struct CommanderExecutionSummary {
    history_note: String,
}

struct CommanderStepOutcome {
    history_notes: Vec<String>,
    speech_notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentHandoff {
    status: HandoffStatus,
    summary: String,
    changed_files: Vec<String>,
    tests: String,
    risks: String,
    next: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandoffStatus {
    Success,
    Questionable,
    Failed,
    NeedsTests,
    Blocked,
}

enum CommanderSlashCommand {
    OpenTheme,
    OpenSettings,
    CreateWorkspace(Option<String>),
    SwitchWorkspace(String),
    ApprovePlan,
    CancelPlan,
}

impl CommanderWorkerResult {
    fn empty() -> Self {
        Self {
            steps: Vec::new(),
            reply_text: None,
            speech_text: None,
            target_name: None,
            payload: None,
            submit_payload: false,
            create_requests: Vec::new(),
            rename_requests: Vec::new(),
            close_requests: Vec::new(),
            workspace_switch: None,
            workspace_create: None,
            proposed_plan: None,
            execution_allowed: true,
        }
    }

    fn into_steps(self) -> Vec<CommanderExecutionStep> {
        if !self.steps.is_empty() {
            return self.steps;
        }
        let has_legacy_fields = self.target_name.is_some()
            || self.reply_text.is_some()
            || self.speech_text.is_some()
            || self.payload.is_some()
            || !self.create_requests.is_empty()
            || !self.rename_requests.is_empty()
            || !self.close_requests.is_empty()
            || self.workspace_switch.is_some()
            || self.workspace_create.is_some();
        if !has_legacy_fields {
            return Vec::new();
        }
        vec![CommanderExecutionStep {
            save_as: None,
            reply_text: self.reply_text,
            speech_text: self.speech_text,
            target_name: self.target_name,
            payload: self.payload,
            submit_payload: self.submit_payload,
            create_requests: self.create_requests,
            rename_requests: self.rename_requests,
            close_requests: self.close_requests,
            workspace_switch: self.workspace_switch,
            workspace_create: self.workspace_create,
        }]
    }
}

struct CreateOutcome {
    note: Option<String>,
    created_ids: Vec<usize>,
}

struct TtsState {
    tx: Option<Sender<String>>,
    event_rx: Option<Receiver<TtsEvent>>,
}

struct CommanderPaletteVideoState {
    child: Option<Child>,
    rx: Option<Receiver<Vec<u8>>>,
    parser: vt100::Parser,
    frame_text: String,
    rows: u16,
    cols: u16,
}

enum TtsEvent {
    SpeakStarted,
    SpeakFinished,
}

#[derive(Clone)]
struct TtsConfig {
    edge_voice: String,
    edge_rate: String,
    edge_volume: String,
    edge_pitch: String,
    timeout_secs: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MousePointerShape {
    Default,
    HorizontalResize,
    VerticalResize,
}

impl App {
    const NEW_PANE_PLACEHOLDER_COMMAND: &'static str = "cat";
    const DUPLICATE_PANE_NAME_ERROR: &'static str = "Name already exists. Pick another.";

    fn pane_name_exists_for_other(&self, pane_id: usize, candidate: &str) -> bool {
        let normalized_candidate = candidate.trim().to_ascii_lowercase();
        if normalized_candidate.is_empty() {
            return false;
        }
        self.panes.iter().any(|pane| {
            pane.id != pane_id && pane.title.trim().to_ascii_lowercase() == normalized_candidate
        })
    }

    pub(crate) fn new(rows: u16, cols: u16) -> anyhow::Result<Self> {
        let content_area = Self::content_area(Rect {
            x: 0,
            y: 0,
            width: cols,
            height: rows,
        });
        let content_rows = content_area.height.saturating_sub(1).max(1);
        let content_cols = content_area.width.saturating_sub(3).max(1);
        let persisted = load_persisted_layout();
        let mut workspaces = persisted
            .as_ref()
            .map(|state| {
                state
                    .workspaces
                    .iter()
                    .map(|workspace| WorkspaceState {
                        name: workspace.name.clone(),
                        layout: workspace.layout.clone(),
                        focused: workspace.focused,
                        maximized_pane: None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if workspaces.is_empty() {
            workspaces.push(WorkspaceState {
                name: "Workspace 1".to_string(),
                layout: Node::Leaf { pane_id: 0 },
                focused: 0,
                maximized_pane: None,
            });
        }

        let mut pane_ids = Vec::new();
        for workspace in &workspaces {
            workspace.layout.collect_leaf_ids(&mut pane_ids);
        }
        pane_ids.sort_unstable();
        pane_ids.dedup();
        if pane_ids.is_empty() {
            pane_ids.push(0);
            if let Some(first_workspace) = workspaces.first_mut() {
                first_workspace.layout = Node::Leaf { pane_id: 0 };
                first_workspace.focused = 0;
                first_workspace.maximized_pane = None;
            }
        }
        for workspace in &mut workspaces {
            if !workspace.layout.contains_pane_id(workspace.focused) {
                workspace.focused = workspace.layout.first_leaf_id();
            }
        }

        let default_agent_index = persisted
            .as_ref()
            .map(|state| state.default_agent_index)
            .unwrap_or(default_agent_index())
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
                let command = if command == COMMANDER_COMMAND {
                    AGENT_PRESETS[default_agent_index].command.to_string()
                } else {
                    command
                };
                let resume_command = persisted
                    .as_ref()
                    .and_then(|state| state.resume_commands.get(&id).cloned());
                let last_command = persisted
                    .as_ref()
                    .and_then(|state| state.last_commands.get(&id).cloned());
                Pane::new(
                    id,
                    title,
                    command,
                    resume_command,
                    last_command,
                    content_rows,
                    content_cols,
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let active_workspace = persisted
            .as_ref()
            .map(|state| {
                state
                    .active_workspace
                    .min(workspaces.len().saturating_sub(1))
            })
            .unwrap_or(0);
        let layout = workspaces[active_workspace].layout.clone();
        let focused = workspaces[active_workspace].focused;
        let next_pane_id = workspaces
            .iter()
            .map(|workspace| workspace.layout.max_leaf_id())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let theme_index =
            load_persisted_theme_index().unwrap_or_else(crate::theme::default_theme_index);

        let mut app = Self {
            panes,
            workspaces,
            active_workspace,
            layout,
            focused,
            maximized_pane: None,
            next_pane_id,
            running: true,
            reload_requested: false,
            modal: None,
            drag_resize: None,
            drag_swap: None,
            drag_pane_mouse: None,
            text_selection: None,
            theme_index,
            default_agent_index,
            theme_preview_index: theme_index,
            debug_container_boxes: parse_debug_flag("SPLIT_TUI_DEBUG_CONTAINERS"),
            mouse_capture_enabled: true,
            commander_focused: false,
            sidebar_workspace_focused: None,
            sidebar_add_button_focused: false,
            commander: CommanderState {
                input: String::new(),
                cursor: 0,
                busy: false,
                phase: CommanderPhase::Discussing,
                pending_plan: None,
                history: vec![
                    "Commander harness ready.".to_string(),
                    "Describe what you want to build — I'll help shape it into a plan.".to_string(),
                    "When the plan looks right, reply with /approve to run it.".to_string(),
                ],
                chat_offset_from_bottom: 0,
                chat_pinned_to_bottom: true,
                chat_viewport_lines: 0,
                chat_total_lines: 0,
                rx: None,
            },
            agent_handoffs: HashMap::new(),
            commander_palette_video: CommanderPaletteVideoState {
                child: None,
                rx: None,
                parser: vt100::Parser::new(14, 32, 0),
                frame_text: String::new(),
                rows: 14,
                cols: 32,
            },
            tts: init_tts_state(),
            last_terminal_size: Rect {
                x: 0,
                y: 0,
                width: cols,
                height: rows,
            },
            hit_test_cache: None,
            last_quit_key_press: None,
        };
        app.rebuild_hit_test_cache();
        Ok(app)
    }

    pub(crate) fn body_area(size: Rect) -> Rect {
        let top_chrome = TOP_CHROME_ROWS.min(size.height);
        Rect {
            x: size.x,
            y: size.y.saturating_add(top_chrome),
            width: size.width,
            height: size.height.saturating_sub(top_chrome),
        }
    }

    pub(crate) fn workspace_sidebar_area(size: Rect) -> Rect {
        crate::ui::top_bar_area(size)
    }

    pub(crate) fn content_area(size: Rect) -> Rect {
        Self::body_area(size)
    }

    fn top_bar_layout(&self, size: Rect, usage_summary: Option<&str>) -> crate::ui::TopBarLayout {
        let workspace_names = self.workspace_names();
        compute_top_bar_layout(
            size,
            &workspace_names,
            self.active_workspace_index(),
            usage_summary,
        )
    }

    pub(crate) fn workspace_names(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect()
    }

    pub(crate) fn workspace_pane_summaries(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .map(|workspace| {
                let mut ids = Vec::new();
                workspace.layout.collect_leaf_ids(&mut ids);
                let count = ids.len();
                let noun = if count == 1 { "Pane" } else { "Panes" };
                format!("{} {}", count, noun)
            })
            .collect()
    }

    pub(crate) fn active_workspace_index(&self) -> usize {
        self.active_workspace
    }

    pub(crate) fn commander_focused(&self) -> bool {
        self.commander_focused
    }

    pub(crate) fn sidebar_workspace_focused(&self) -> Option<usize> {
        self.sidebar_workspace_focused
    }

    pub(crate) fn sidebar_add_button_focused(&self) -> bool {
        self.sidebar_add_button_focused
    }

    fn sync_active_workspace_state(&mut self) {
        if let Some(workspace) = self.workspaces.get_mut(self.active_workspace) {
            workspace.layout = self.layout.clone();
            workspace.focused = self.focused;
            workspace.maximized_pane = self.maximized_pane;
        }
    }

    fn load_active_workspace_state(&mut self) {
        let Some(workspace) = self.workspaces.get(self.active_workspace) else {
            return;
        };
        self.layout = workspace.layout.clone();
        self.focused = workspace.focused;
        self.maximized_pane = workspace.maximized_pane;
        if !self.layout.contains_pane_id(self.focused) {
            self.focused = self.layout.first_leaf_id();
        }
        if self
            .maximized_pane
            .is_some_and(|pane_id| !self.layout.contains_pane_id(pane_id))
        {
            self.maximized_pane = None;
        }
    }

    pub(crate) fn resize(&mut self, total_rows: u16, cols: u16) {
        self.last_terminal_size = Rect {
            x: 0,
            y: 0,
            width: cols,
            height: total_rows,
        };
        let placements = self.pane_placements(Self::content_area(Rect {
            x: 0,
            y: 0,
            width: cols,
            height: total_rows,
        }));

        for placement in placements {
            if let Some(pane) = self.pane_mut(placement.pane_id) {
                let inner = pane_inner_area(placement.area, placement.exposed);
                let content_cols = inner.width.max(1);
                let content_rows = inner.height.max(1);
                pane.resize(content_rows, content_cols);
            }
        }
        self.rebuild_hit_test_cache();
    }

    /// Drain PTY output for all panes. Returns true if any pane processed new
    /// bytes (i.e. the screen may need to be redrawn). Panes whose program
    /// has exited are automatically respawned as a login shell so the user
    /// can keep using the slot instead of staring at a frozen view.
    pub(crate) fn tick(&mut self) -> bool {
        let mut any = false;
        if self.poll_tts_events() {
            any = true;
        }
        if self.poll_commander_result() {
            any = true;
        }
        if self.poll_commander_palette_video() {
            any = true;
        }
        let mut handoff_updates = Vec::new();
        for pane in &mut self.panes {
            if pane.pump() {
                if pane.command != COMMANDER_COMMAND {
                    let text = pane.recent_plain_text();
                    if let Some(handoff) = parse_agent_handoff(&text) {
                        handoff_updates.push((pane.id, handoff));
                    }
                }
                any = true;
            }
            if pane.exited && !pane.relaunch_failed {
                match pane.relaunch_as_shell() {
                    Ok(()) => any = true,
                    Err(_) => pane.relaunch_failed = true,
                }
            }
        }
        for (pane_id, handoff) in handoff_updates {
            self.agent_handoffs.insert(pane_id, handoff);
        }
        any
    }

    /// Graceful detach: drain output briefly and take a final best-effort
    /// snapshot. Pane processes live in persistent tmux sessions, so quitting
    /// or reloading the app must not signal the program running inside them.
    pub(crate) fn shutdown_panes(&mut self, max_wait: Duration) {
        self.stop_commander_palette_video();
        let deadline = Instant::now() + max_wait;
        loop {
            let mut any = false;
            for pane in &mut self.panes {
                any |= pane.pump();
            }
            if !any || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        for pane in &mut self.panes {
            if pane.resume_command.is_none() {
                pane.try_capture_resume_command();
            }
        }
    }

    pub(crate) fn persist_layout(&mut self) {
        self.sync_active_workspace_state();
        let persisted_workspaces = self
            .workspaces
            .iter()
            .map(|workspace| PersistedWorkspace {
                name: workspace.name.clone(),
                layout: workspace.layout.clone(),
                focused: workspace.focused,
            })
            .collect::<Vec<_>>();
        let _ = save_persisted_layout(
            &persisted_workspaces,
            self.active_workspace,
            self.default_agent_index,
            &self.panes,
        );
    }

    fn switch_workspace(&mut self, workspace_index: usize, size: Rect) {
        if workspace_index >= self.workspaces.len() {
            return;
        }
        if workspace_index == self.active_workspace {
            self.commander_focused = false;
            self.sidebar_workspace_focused = None;
            self.sidebar_add_button_focused = false;
            self.drag_resize = None;
            self.drag_swap = None;
            self.drag_pane_mouse = None;
            self.resize(size.height, size.width);
            self.persist_layout();
            return;
        }
        self.sync_active_workspace_state();
        self.active_workspace = workspace_index;
        self.load_active_workspace_state();
        self.drag_resize = None;
        self.drag_swap = None;
        self.drag_pane_mouse = None;
        self.commander_focused = false;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.resize(size.height, size.width);
        self.persist_layout();
    }

    fn switch_workspace_from_request(&mut self, selector: &str) -> Result<String, String> {
        let Some(workspace_index) = self.resolve_workspace_selector(selector) else {
            return Err(format!("Couldn't find workspace {}.", selector.trim()));
        };
        let name = self
            .workspaces
            .get(workspace_index)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| format!("Workspace {}", workspace_index + 1));
        self.switch_workspace(workspace_index, self.last_terminal_size);
        Ok(format!("Switched to {}.", name))
    }

    fn create_workspace_from_request(&mut self, name: Option<&str>) -> Result<String, String> {
        self.create_workspace(self.last_terminal_size)
            .map_err(|_| "Couldn't create a new workspace.".to_string())?;

        let workspace_index = self.active_workspace;
        let mut workspace_name = self
            .workspaces
            .get(workspace_index)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| format!("Workspace {}", workspace_index + 1));

        if let Some(name) = name.map(str::trim).filter(|name| {
            !name.is_empty()
                && !name.eq_ignore_ascii_case("none")
                && !name.eq_ignore_ascii_case("new")
        }) {
            if self.workspaces.iter().enumerate().any(|(idx, workspace)| {
                idx != workspace_index && workspace.name.trim().eq_ignore_ascii_case(name)
            }) {
                return Ok(format!(
                    "Created {}, but kept the default name because {} already exists.",
                    workspace_name, name
                ));
            }

            match self.rename_workspace(workspace_index, name.to_string()) {
                Ok(()) => workspace_name = name.to_string(),
                Err(_) => {
                    return Ok(format!(
                        "Created {}, but couldn't apply that workspace name.",
                        workspace_name
                    ));
                }
            }
        }

        Ok(format!("Created workspace {}.", workspace_name))
    }

    fn resolve_workspace_selector(&self, selector: &str) -> Option<usize> {
        let raw = selector.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
            return None;
        }
        let normalized = raw.to_ascii_lowercase();
        match normalized.as_str() {
            "current" | "active" => return Some(self.active_workspace),
            "next" => {
                return (!self.workspaces.is_empty())
                    .then_some((self.active_workspace + 1) % self.workspaces.len());
            }
            "previous" | "prev" | "last" => {
                return (!self.workspaces.is_empty()).then_some(
                    (self.active_workspace + self.workspaces.len().saturating_sub(1))
                        % self.workspaces.len(),
                );
            }
            _ => {}
        }

        let numeric = normalized
            .strip_prefix("workspace ")
            .or_else(|| normalized.strip_prefix("workspace:"))
            .or_else(|| normalized.strip_prefix("id:"))
            .unwrap_or(&normalized)
            .trim();
        if let Ok(one_based) = numeric.parse::<usize>() {
            if (1..=self.workspaces.len()).contains(&one_based) {
                return Some(one_based - 1);
            }
            if one_based < self.workspaces.len() {
                return Some(one_based);
            }
        }

        self.workspaces
            .iter()
            .position(|workspace| workspace.name.trim().eq_ignore_ascii_case(raw))
            .or_else(|| {
                self.workspaces.iter().position(|workspace| {
                    workspace
                        .name
                        .to_ascii_lowercase()
                        .contains(normalized.as_str())
                })
            })
    }

    fn create_workspace(&mut self, size: Rect) -> anyhow::Result<()> {
        self.sync_active_workspace_state();
        let pane_id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);

        let title = format!("Pane {}", pane_id + 1);
        let default_index = self.first_available_agent_for_pane(pane_id, self.default_agent_index);
        self.panes.push(Pane::new(
            pane_id,
            title,
            AGENT_PRESETS[default_index].command,
            None,
            None,
            1,
            1,
        )?);

        self.workspaces.push(WorkspaceState {
            name: format!("Workspace {}", self.workspaces.len() + 1),
            layout: Node::Leaf { pane_id },
            focused: pane_id,
            maximized_pane: None,
        });
        self.active_workspace = self.workspaces.len().saturating_sub(1);
        self.load_active_workspace_state();
        self.commander_focused = false;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.drag_pane_mouse = None;
        self.resize(size.height, size.width);
        self.persist_layout();
        Ok(())
    }

    fn open_workspace_settings_modal(&mut self, workspace_index: usize) {
        let Some(workspace) = self.workspaces.get(workspace_index) else {
            return;
        };
        self.modal = Some(Modal::WorkspaceSettings {
            workspace_index,
            name: workspace.name.clone(),
            name_error: None,
            cursor: workspace.name.chars().count(),
            action_index: 0,
        });
    }

    fn rename_workspace(
        &mut self,
        workspace_index: usize,
        name: String,
    ) -> Result<(), &'static str> {
        if workspace_index >= self.workspaces.len() {
            return Err("Workspace no longer exists.");
        }
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("Name cannot be empty.");
        }
        self.workspaces[workspace_index].name = trimmed.to_string();
        self.persist_layout();
        Ok(())
    }

    fn close_workspace(&mut self, workspace_index: usize, size: Rect) -> Result<(), &'static str> {
        if self.workspaces.len() <= 1 {
            return Err("At least one workspace is required.");
        }
        if workspace_index >= self.workspaces.len() {
            return Err("Workspace no longer exists.");
        }

        self.sync_active_workspace_state();
        self.workspaces.remove(workspace_index);

        let mut referenced_pane_ids = HashSet::new();
        for workspace in &self.workspaces {
            let mut ids = Vec::new();
            workspace.layout.collect_leaf_ids(&mut ids);
            referenced_pane_ids.extend(ids);
        }
        self.panes.retain(|pane| {
            let keep = referenced_pane_ids.contains(&pane.id);
            if !keep {
                pane.terminate_session();
            }
            keep
        });

        if workspace_index < self.active_workspace {
            self.active_workspace = self.active_workspace.saturating_sub(1);
        } else if workspace_index == self.active_workspace
            && self.active_workspace >= self.workspaces.len()
        {
            self.active_workspace = self.workspaces.len().saturating_sub(1);
        }

        self.load_active_workspace_state();
        self.drag_resize = None;
        self.drag_swap = None;
        self.drag_pane_mouse = None;
        self.commander_focused = false;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.resize(size.height, size.width);
        self.persist_layout();
        Ok(())
    }

    fn handle_workspace_sidebar_click(
        &mut self,
        size: Rect,
        x: u16,
        y: u16,
    ) -> anyhow::Result<bool> {
        let sidebar = Self::workspace_sidebar_area(size);
        if sidebar.width == 0 || sidebar.height == 0 || !contains(sidebar, x, y) {
            return Ok(false);
        }

        let workspace_names: Vec<String> = self.workspace_names();
        if let Some(workspace_index) =
            workspace_menu_hit_index(sidebar, &workspace_names, self.active_workspace, x, y)
        {
            self.open_workspace_settings_modal(workspace_index);
            return Ok(true);
        }

        if let Some(workspace_index) =
            workspace_hit_index(sidebar, &workspace_names, self.active_workspace, x, y)
        {
            self.switch_workspace(workspace_index, size);
            return Ok(true);
        }
        if workspace_add_button_hit(sidebar, &workspace_names, self.active_workspace, x, y) {
            self.create_workspace(size)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn focus_commander_from_sidebar(&mut self, size: Rect) {
        self.commander_focused = true;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.drag_resize = None;
        self.drag_swap = None;
        self.drag_pane_mouse = None;
        self.resize(size.height, size.width);
        self.persist_layout();
    }

    fn focus_workspace_tab_from_sidebar(&mut self, workspace_index: usize) {
        if workspace_index >= self.workspaces.len() {
            return;
        }
        self.commander_focused = false;
        self.sidebar_workspace_focused = Some(workspace_index);
        self.sidebar_add_button_focused = false;
    }

    fn focus_workspace_add_button_from_sidebar(&mut self) {
        self.commander_focused = false;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = true;
    }

    fn activate_sidebar_workspace(&mut self, size: Rect) {
        let Some(workspace_index) = self.sidebar_workspace_focused else {
            return;
        };
        self.switch_workspace(workspace_index, size);
        self.commander_focused = false;
        self.sidebar_workspace_focused = Some(workspace_index);
        self.sidebar_add_button_focused = false;
    }

    fn activate_sidebar_add_button(&mut self, size: Rect) -> anyhow::Result<()> {
        if !self.sidebar_add_button_focused {
            return Ok(());
        }
        self.create_workspace(size)
    }

    fn sidebar_item_count(&self) -> usize {
        2 + self.workspaces.len()
    }

    fn sidebar_add_button_index(&self) -> usize {
        self.workspaces.len().saturating_add(1)
    }

    fn sidebar_item_index(&self) -> Option<usize> {
        if self.commander_focused {
            Some(0)
        } else if self.sidebar_add_button_focused {
            Some(self.sidebar_add_button_index())
        } else {
            self.sidebar_workspace_focused
                .map(|workspace_index| workspace_index.saturating_add(1))
        }
    }

    fn focus_sidebar_item(&mut self, size: Rect, item_index: usize) {
        if item_index == 0 {
            self.focus_commander_from_sidebar(size);
            return;
        }
        if item_index == self.sidebar_add_button_index() {
            self.focus_workspace_add_button_from_sidebar();
            return;
        }
        self.focus_workspace_tab_from_sidebar(item_index.saturating_sub(1));
    }

    fn move_sidebar_focus(&mut self, size: Rect, step: isize) {
        let total = self.sidebar_item_count();
        let Some(current) = self.sidebar_item_index() else {
            return;
        };
        let next = shift_sidebar_item_index(current, total, step);
        if next != current {
            self.focus_sidebar_item(size, next);
        }
    }

    fn sidebar_is_visible(size: Rect) -> bool {
        let sidebar = Self::workspace_sidebar_area(size);
        sidebar.width > 0 && sidebar.height > 0
    }

    fn handle_ctrl_arrow_focus(&mut self, size: Rect, side: SplitSide) {
        if self.commander_focused
            || self.sidebar_workspace_focused.is_some()
            || self.sidebar_add_button_focused
        {
            match side {
                SplitSide::Left => self.move_sidebar_focus(size, -1),
                SplitSide::Right => self.move_sidebar_focus(size, 1),
                SplitSide::Bottom => self.focus_pane(self.focused),
                SplitSide::Top => {}
            }
            return;
        }

        let moved = self.focus_adjacent(size, side);
        if !moved && side == SplitSide::Left {
            self.focus_commander_from_sidebar(size);
        } else if !moved && side == SplitSide::Top && Self::sidebar_is_visible(size) {
            self.focus_workspace_tab_from_sidebar(self.active_workspace);
        }
    }

    fn pane_inner_rect(&self, size: Rect, pane_id: usize) -> Option<Rect> {
        self.pane_placements(Self::content_area(size))
            .into_iter()
            .find(|placement| placement.pane_id == pane_id)
            .map(|placement| pane_inner_area(placement.area, placement.exposed))
    }

    fn pane_mouse_cell(inner: Rect, mouse_column: u16, mouse_row: u16) -> Option<(u16, u16)> {
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let x = mouse_column
            .saturating_sub(inner.x)
            .min(inner.width.saturating_sub(1));
        let y = mouse_row
            .saturating_sub(inner.y)
            .min(inner.height.saturating_sub(1));
        Some((x, y))
    }

    fn send_mouse_button_to_pane(
        &mut self,
        pane_id: usize,
        inner: Rect,
        event_kind: PaneMouseEventKind,
        button: MouseButton,
        modifiers: KeyModifiers,
        mouse_column: u16,
        mouse_row: u16,
    ) -> anyhow::Result<bool> {
        let Some((x, y)) = Self::pane_mouse_cell(inner, mouse_column, mouse_row) else {
            return Ok(false);
        };
        let Some(pane) = self.pane_mut(pane_id) else {
            return Ok(false);
        };
        pane.send_mouse_button(button, event_kind, modifiers, x, y)
    }

    fn start_text_selection(&mut self, pane_id: usize, cell: (u16, u16)) {
        self.text_selection = Some(TextSelection {
            pane_id,
            start: cell,
            end: cell,
            active: true,
        });
        self.drag_pane_mouse = None;
    }

    fn update_text_selection(&mut self, size: Rect, mouse: &MouseEvent) -> bool {
        let Some(mut selection) = self.text_selection else {
            return false;
        };
        if !selection.active {
            return false;
        }

        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(inner) = self.pane_inner_rect(size, selection.pane_id) else {
                    self.text_selection = None;
                    return false;
                };
                if let Some(cell) = Self::pane_mouse_cell(inner, mouse.column, mouse.row) {
                    selection.end = cell;
                    self.text_selection = Some(selection);
                    return true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if selection.start == selection.end {
                    self.text_selection = None;
                    return true;
                }
                selection.active = false;
                self.text_selection = Some(selection);
                return true;
            }
            _ => {}
        }

        false
    }

    fn copy_text_selection(&mut self) -> anyhow::Result<bool> {
        let Some(selection) = self.text_selection else {
            return Ok(false);
        };
        if selection.start == selection.end {
            return Ok(false);
        }
        let Some(pane) = self.pane(selection.pane_id) else {
            return Ok(false);
        };
        let text = pane.selected_text(PaneSelection {
            start: selection.start,
            end: selection.end,
        });
        if text.is_empty() {
            return Ok(false);
        }
        write_osc52_clipboard(&text)?;
        Ok(true)
    }

    fn move_keyboard_selection(&mut self, delta: i16) -> bool {
        let pane_id = self.focused;
        let Some(pane) = self.pane(pane_id) else {
            return false;
        };
        let Some(cursor) = pane.cursor_cell() else {
            return false;
        };
        let cols = pane.cols;
        let rows = pane.rows;
        if cols == 0 || rows == 0 {
            return false;
        }

        let mut selection = if let Some(selection) = self
            .text_selection
            .filter(|selection| selection.pane_id == pane_id)
        {
            let mut selection = selection;
            selection.end = move_cell(selection.end, delta, cols, rows);
            selection
        } else {
            let selected = if delta.is_negative() {
                move_cell(cursor, delta, cols, rows)
            } else {
                cursor
            };
            TextSelection {
                pane_id,
                start: selected,
                end: selected,
                active: false,
            }
        };
        selection.active = false;
        self.text_selection = Some(selection);
        true
    }

    pub(crate) fn pane_selection(&self, pane_id: usize) -> Option<PaneSelection> {
        self.text_selection
            .filter(|selection| selection.pane_id == pane_id)
            .map(|selection| PaneSelection {
                start: selection.start,
                end: selection.end,
            })
    }

    fn forward_active_pane_mouse_drag(
        &mut self,
        size: Rect,
        mouse: &MouseEvent,
    ) -> anyhow::Result<bool> {
        let Some(active_drag) = self.drag_pane_mouse else {
            return Ok(false);
        };

        let Some(inner) = self.pane_inner_rect(size, active_drag.pane_id) else {
            self.drag_pane_mouse = None;
            return Ok(false);
        };

        match mouse.kind {
            MouseEventKind::Drag(_) => {
                let _ = self.send_mouse_button_to_pane(
                    active_drag.pane_id,
                    inner,
                    PaneMouseEventKind::Drag,
                    active_drag.button,
                    mouse.modifiers,
                    mouse.column,
                    mouse.row,
                )?;
                Ok(true)
            }
            MouseEventKind::Up(_) => {
                let _ = self.send_mouse_button_to_pane(
                    active_drag.pane_id,
                    inner,
                    PaneMouseEventKind::Up,
                    active_drag.button,
                    mouse.modifiers,
                    mouse.column,
                    mouse.row,
                )?;
                self.drag_pane_mouse = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn close_pane(&mut self) {
        let mut current_workspace_panes = Vec::new();
        self.layout.collect_leaf_ids(&mut current_workspace_panes);
        if current_workspace_panes.len() <= 1 {
            return;
        }

        let focused = self.focused;
        let Some(pos) = self.panes.iter().position(|pane| pane.id == focused) else {
            return;
        };

        let Some(next_focus) = self.layout.delete_leaf(focused) else {
            return;
        };

        self.panes[pos].terminate_session();
        self.panes.remove(pos);
        self.focus_pane(next_focus);
        for (workspace_index, workspace) in self.workspaces.iter_mut().enumerate() {
            if workspace_index == self.active_workspace
                || !workspace.layout.contains_pane_id(focused)
            {
                continue;
            }

            let mut pane_ids = Vec::new();
            workspace.layout.collect_leaf_ids(&mut pane_ids);
            if pane_ids.len() <= 1 {
                workspace.layout = Node::Leaf {
                    pane_id: next_focus,
                };
                workspace.focused = next_focus;
                workspace.maximized_pane = None;
                continue;
            }

            if let Some(other_focus) = workspace.layout.delete_leaf(focused) {
                if workspace.focused == focused {
                    workspace.focused = other_focus;
                }
            }
            if workspace.maximized_pane == Some(focused) {
                workspace.maximized_pane = None;
            }
        }
        self.persist_layout();
        self.resize(
            self.last_terminal_size.height,
            self.last_terminal_size.width,
        );
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
        if self.pane_name_exists_for_other(pane_id, &name) {
            return Ok(());
        }

        let command = AGENT_PRESETS
            .get(agent_index)
            .map(|p| p.command)
            .unwrap_or(AGENT_PRESETS[0].command)
            .to_string();
        if !self.command_available_for_pane(pane_id, &command) {
            return Ok(());
        }

        let (rows, cols, command_changed, title_changed) = {
            let pane = &self.panes[pos];
            (
                pane.rows,
                pane.cols,
                pane.command != command,
                pane.title != name,
            )
        };

        if !command_changed && !title_changed {
            return Ok(());
        }

        if !command_changed {
            self.panes[pos].title = name;
            self.focus_pane(pane_id);
            self.persist_layout();
            return Ok(());
        }

        // Changing the agent of a pane discards any prior session state.
        self.panes[pos].terminate_session();
        self.panes[pos] = Pane::new(pane_id, name, command, None, None, rows, cols)?;
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
            self.drag_swap = None;
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            let has_text_selection = self
                .text_selection
                .is_some_and(|selection| selection.start != selection.end);
            if self.copy_text_selection()? || has_text_selection {
                return Ok(());
            }
            if !self.focused_pane_is_commander() {
                if let Some(pane) = self.focused_pane_mut() {
                    pane.send(&[0x03])?;
                    pane.clear_pending_input();
                }
            }
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W'))
        {
            self.close_pane();
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
                        self.rebuild_hit_test_cache();
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
                Modal::NewPanePicker {
                    pane_id,
                    source_pane_id,
                    close_on_cancel,
                    mut name,
                    mut name_error,
                    mut cursor,
                    mut name_selected,
                    mut agent_index,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            if close_on_cancel {
                                self.focus_pane(pane_id);
                                self.close_pane();
                                self.focus_pane(source_pane_id);
                            }
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Up | KeyCode::Left => {
                            if key.code == KeyCode::Up {
                                agent_index =
                                    self.cycle_available_agent_for_pane(pane_id, agent_index, -1);
                            } else {
                                if name_selected {
                                    cursor = 0;
                                    name_selected = false;
                                } else {
                                    cursor = cursor.saturating_sub(1);
                                }
                            }
                        }
                        KeyCode::Down | KeyCode::Right => {
                            if key.code == KeyCode::Down {
                                agent_index =
                                    self.cycle_available_agent_for_pane(pane_id, agent_index, 1);
                            } else {
                                if name_selected {
                                    cursor = name.chars().count();
                                    name_selected = false;
                                } else {
                                    cursor = (cursor + 1).min(name.chars().count());
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if name_selected {
                                name.clear();
                                cursor = 0;
                                name_selected = false;
                            } else {
                                remove_char_before_cursor(&mut name, &mut cursor);
                            }
                            name_error = None;
                        }
                        KeyCode::Delete => {
                            if name_selected {
                                name.clear();
                                cursor = 0;
                                name_selected = false;
                            } else {
                                remove_char_at_cursor(&mut name, cursor);
                            }
                            name_error = None;
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if name_selected {
                                name.clear();
                                cursor = 0;
                                name_selected = false;
                            }
                            insert_char_at_cursor(&mut name, &mut cursor, c);
                            name_error = None;
                        }
                        KeyCode::Enter => {
                            if !self.agent_available_for_pane(pane_id, agent_index) {
                                self.modal = Some(Modal::NewPanePicker {
                                    pane_id,
                                    source_pane_id,
                                    close_on_cancel,
                                    name,
                                    name_error,
                                    cursor,
                                    name_selected,
                                    agent_index,
                                });
                                return Ok(());
                            }
                            if self.pane_name_exists_for_other(pane_id, &name) {
                                self.modal = Some(Modal::NewPanePicker {
                                    pane_id,
                                    source_pane_id,
                                    close_on_cancel,
                                    name,
                                    name_error: Some(Self::DUPLICATE_PANE_NAME_ERROR.to_string()),
                                    cursor,
                                    name_selected,
                                    agent_index,
                                });
                                return Ok(());
                            }
                            self.default_agent_index = agent_index;
                            self.apply_panel_settings(pane_id, name, agent_index)?;
                            self.modal = None;
                            return Ok(());
                        }
                        _ => {}
                    }

                    self.modal = Some(Modal::NewPanePicker {
                        pane_id,
                        source_pane_id,
                        close_on_cancel,
                        name,
                        name_error,
                        cursor,
                        name_selected,
                        agent_index,
                    });
                    return Ok(());
                }
                Modal::PanelSettings {
                    pane_id,
                    mut name,
                    mut name_error,
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
                            if !self.agent_available_for_pane(pane_id, agent_index) {
                                self.modal = Some(Modal::PanelSettings {
                                    pane_id,
                                    name,
                                    name_error,
                                    agent_index,
                                    focus,
                                });
                                return Ok(());
                            }
                            if self.pane_name_exists_for_other(pane_id, &name) {
                                self.modal = Some(Modal::PanelSettings {
                                    pane_id,
                                    name,
                                    name_error: Some(Self::DUPLICATE_PANE_NAME_ERROR.to_string()),
                                    agent_index,
                                    focus,
                                });
                                return Ok(());
                            }
                            self.apply_panel_settings(pane_id, name, agent_index)?;
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Backspace if focus == PanelSettingsFocus::Name => {
                            name.pop();
                            name_error = None;
                        }
                        KeyCode::Char(c)
                            if focus == PanelSettingsFocus::Name
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            name.push(c);
                            name_error = None;
                        }
                        KeyCode::Left | KeyCode::Up if focus == PanelSettingsFocus::Agent => {
                            agent_index =
                                self.cycle_available_agent_for_pane(pane_id, agent_index, -1);
                        }
                        KeyCode::Right | KeyCode::Down if focus == PanelSettingsFocus::Agent => {
                            agent_index =
                                self.cycle_available_agent_for_pane(pane_id, agent_index, 1);
                        }
                        _ => {}
                    }

                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        name_error,
                        agent_index,
                        focus,
                    });
                    return Ok(());
                }
                Modal::WorkspaceSettings {
                    workspace_index,
                    mut name,
                    mut name_error,
                    mut cursor,
                    mut action_index,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            self.modal = None;
                            return Ok(());
                        }
                        KeyCode::Up => {
                            action_index = (action_index + 2) % 3;
                        }
                        KeyCode::Down => {
                            action_index = (action_index + 1) % 3;
                        }
                        KeyCode::Left => {
                            cursor = cursor.saturating_sub(1);
                        }
                        KeyCode::Right => {
                            cursor = (cursor + 1).min(name.chars().count());
                        }
                        KeyCode::Home => {
                            cursor = 0;
                        }
                        KeyCode::End => {
                            cursor = name.chars().count();
                        }
                        KeyCode::Backspace => {
                            remove_char_before_cursor(&mut name, &mut cursor);
                            name_error = None;
                        }
                        KeyCode::Delete => {
                            remove_char_at_cursor(&mut name, cursor);
                            name_error = None;
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            insert_char_at_cursor(&mut name, &mut cursor, c);
                            name_error = None;
                        }
                        KeyCode::Enter => match action_index.min(2) {
                            0 => match self.rename_workspace(workspace_index, name.clone()) {
                                Ok(()) => {
                                    self.modal = None;
                                    return Ok(());
                                }
                                Err(error) => {
                                    name_error = Some(error.to_string());
                                }
                            },
                            1 => {
                                self.modal = None;
                                return Ok(());
                            }
                            _ => match self.close_workspace(workspace_index, size) {
                                Ok(()) => {
                                    self.modal = None;
                                    return Ok(());
                                }
                                Err(error) => {
                                    name_error = Some(error.to_string());
                                }
                            },
                        },
                        _ => {}
                    }
                    self.modal = Some(Modal::WorkspaceSettings {
                        workspace_index,
                        name,
                        name_error,
                        cursor,
                        action_index,
                    });
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
                let source_pane_id = self.focused;
                let pane_id = self.split_pane_with_command(
                    source_pane_id,
                    side,
                    Self::NEW_PANE_PLACEHOLDER_COMMAND,
                    size,
                )?;
                self.focus_pane(pane_id);
                let pane_name = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .map(|pane| pane.title.clone())
                    .unwrap_or_else(|| format!("Pane {}", pane_id + 1));
                let agent_index =
                    self.first_available_agent_for_pane(pane_id, self.default_agent_index);
                self.modal = Some(Modal::NewPanePicker {
                    pane_id,
                    source_pane_id,
                    close_on_cancel: true,
                    cursor: pane_name.chars().count(),
                    name_selected: true,
                    name: pane_name,
                    name_error: None,
                    agent_index,
                });
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
                self.handle_ctrl_arrow_focus(size, side);
                return Ok(());
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            let now = Instant::now();
            if self
                .last_quit_key_press
                .is_some_and(|t| now.duration_since(t) <= QUIT_CONFIRM_WINDOW)
            {
                self.running = false;
            } else {
                self.last_quit_key_press = Some(now);
            }
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::SHIFT) {
            if !self.focused_pane_is_commander() {
                match key.code {
                    KeyCode::Left => {
                        if self.move_keyboard_selection(-1) {
                            return Ok(());
                        }
                    }
                    KeyCode::Right => {
                        if self.move_keyboard_selection(1) {
                            return Ok(());
                        }
                    }
                    _ => {}
                }

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
        }

        if !self.focused_pane_is_commander()
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
        {
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

        if self.focused_pane_is_commander() {
            match key.code {
                KeyCode::PageUp => {
                    self.commander_page_up();
                    return Ok(());
                }
                KeyCode::PageDown => {
                    self.commander_page_down();
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.commander_submit_current_input();
                }
                KeyCode::Left => {
                    self.commander.cursor = self.commander.cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    self.commander.cursor =
                        (self.commander.cursor + 1).min(self.commander.input.chars().count());
                }
                KeyCode::Home => {
                    self.commander.cursor = 0;
                }
                KeyCode::End => {
                    self.commander.cursor = self.commander.input.chars().count();
                }
                KeyCode::Backspace => {
                    remove_char_before_cursor(
                        &mut self.commander.input,
                        &mut self.commander.cursor,
                    );
                }
                KeyCode::Delete => {
                    remove_char_at_cursor(&mut self.commander.input, self.commander.cursor);
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    insert_char_at_cursor(&mut self.commander.input, &mut self.commander.cursor, c);
                }
                _ => {}
            }
            return Ok(());
        }

        if self.sidebar_workspace_focused.is_some() {
            if key.code == KeyCode::Enter {
                self.activate_sidebar_workspace(size);
            }
            return Ok(());
        }

        if self.sidebar_add_button_focused {
            if key.code == KeyCode::Enter {
                self.activate_sidebar_add_button(size)?;
            }
            return Ok(());
        }

        let bytes = key_to_bytes(key.clone());
        if !bytes.is_empty() {
            if let Some(pane) = self.focused_pane_mut() {
                pane.send(&bytes)?;
                pane.track_key_event(key);
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
                    name_error: _,
                    agent_index,
                    focus,
                } if focus == PanelSettingsFocus::Name => {
                    name.push_str(&text);
                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        name_error: None,
                        agent_index,
                        focus,
                    });
                }
                Modal::NewPanePicker {
                    pane_id,
                    source_pane_id,
                    close_on_cancel,
                    mut name,
                    name_error: _,
                    mut cursor,
                    mut name_selected,
                    agent_index,
                } => {
                    if name_selected {
                        name.clear();
                        cursor = 0;
                        name_selected = false;
                    }
                    for ch in text.chars() {
                        insert_char_at_cursor(&mut name, &mut cursor, ch);
                    }
                    self.modal = Some(Modal::NewPanePicker {
                        pane_id,
                        source_pane_id,
                        close_on_cancel,
                        name,
                        name_error: None,
                        cursor,
                        name_selected,
                        agent_index,
                    });
                }
                Modal::WorkspaceSettings {
                    workspace_index,
                    mut name,
                    name_error: _,
                    mut cursor,
                    action_index,
                } => {
                    for ch in text.chars() {
                        insert_char_at_cursor(&mut name, &mut cursor, ch);
                    }
                    self.modal = Some(Modal::WorkspaceSettings {
                        workspace_index,
                        name,
                        name_error: None,
                        cursor,
                        action_index,
                    });
                }
                other => {
                    self.modal = Some(other);
                }
            }
            return Ok(());
        }

        if self.focused_pane_is_commander() {
            for ch in text.chars() {
                insert_char_at_cursor(&mut self.commander.input, &mut self.commander.cursor, ch);
            }
            self.commander_submit_current_input();
        } else if self.sidebar_workspace_focused.is_some() || self.sidebar_add_button_focused {
            return Ok(());
        } else if let Some(pane) = self.focused_pane_mut() {
            pane.send_paste(&text)?;
            pane.track_paste(&text);
        }

        Ok(())
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent, size: Rect) -> anyhow::Result<()> {
        // Plain mouse move events can flood the queue and create visible click
        // latency. Ignore them unless an active drag/selection needs updates.
        if matches!(mouse.kind, MouseEventKind::Moved)
            && self.drag_resize.is_none()
            && self.drag_swap.is_none()
            && self.drag_pane_mouse.is_none()
            && !self
                .text_selection
                .is_some_and(|selection| selection.active)
        {
            return Ok(());
        }

        if self.update_text_selection(size, &mouse) {
            return Ok(());
        }

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

        if let Some(mut drag) = self.drag_swap.take() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    drag.moved = true;
                    drag.hovered_pane_id =
                        self.update_pane_swap_hover_target(size, drag.source_pane_id, &mouse);
                    self.drag_swap = Some(drag);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.finish_pane_swap_drag(drag, size)?;
                    return Ok(());
                }
                _ => {
                    self.drag_swap = Some(drag);
                }
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
                            self.rebuild_hit_test_cache();
                        }
                    }
                    self.modal = Some(Modal::Help);
                    return Ok(());
                }
                Modal::Theme => {
                    self.modal = Some(Modal::Theme);
                    return Ok(());
                }
                Modal::NewPanePicker {
                    pane_id,
                    source_pane_id,
                    close_on_cancel,
                    name,
                    name_error,
                    mut cursor,
                    mut name_selected,
                    mut agent_index,
                } => {
                    let (pane_area, anchor_title) = self
                        .pane_placements(Self::content_area(size))
                        .into_iter()
                        .find(|placement| placement.pane_id == pane_id)
                        .and_then(|placement| {
                            self.panes
                                .iter()
                                .find(|pane| pane.id == pane_id)
                                .map(|pane| {
                                    (
                                        placement.area,
                                        pane_chrome_title_label(&pane.title, &pane.command),
                                    )
                                })
                        })
                        .unwrap_or_else(|| (Self::content_area(size), "Pane".to_string()));
                    let area = new_pane_picker_modal_area(pane_area, &anchor_title);
                    let name_area = new_pane_picker_name_input_area(area);
                    let name_inner = ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .inner(name_area);
                    let list_area = new_pane_picker_list_area(area);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if contains(list_area, mouse.column, mouse.row) {
                            let selected = mouse.row.saturating_sub(list_area.y) as usize;
                            if selected < AGENT_PRESETS.len()
                                && self.agent_available_for_pane(pane_id, selected)
                            {
                                agent_index = selected;
                            }
                        } else if contains(name_inner, mouse.column, mouse.row) {
                            let click_col = mouse.column.saturating_sub(name_inner.x) as usize;
                            cursor = click_col.min(name.chars().count());
                            name_selected = false;
                        }
                    }
                    self.modal = Some(Modal::NewPanePicker {
                        pane_id,
                        source_pane_id,
                        close_on_cancel,
                        name,
                        name_error,
                        cursor,
                        name_selected,
                        agent_index,
                    });
                    return Ok(());
                }
                Modal::PanelSettings {
                    pane_id,
                    name,
                    name_error,
                    mut agent_index,
                    mut focus,
                } => {
                    let (pane_area, anchor_title) = self
                        .pane_placements(Self::content_area(size))
                        .into_iter()
                        .find(|placement| placement.pane_id == pane_id)
                        .and_then(|placement| {
                            self.panes
                                .iter()
                                .find(|pane| pane.id == pane_id)
                                .map(|pane| {
                                    (
                                        placement.area,
                                        pane_chrome_title_label(&pane.title, &pane.command),
                                    )
                                })
                        })
                        .unwrap_or_else(|| (Self::content_area(size), "Pane".to_string()));
                    let area = panel_settings_modal_area(pane_area, &anchor_title);
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
                            if !self.agent_available_for_pane(pane_id, agent_index) {
                                self.modal = Some(Modal::PanelSettings {
                                    pane_id,
                                    name,
                                    name_error,
                                    agent_index,
                                    focus,
                                });
                                return Ok(());
                            }
                            if self.pane_name_exists_for_other(pane_id, &name) {
                                self.modal = Some(Modal::PanelSettings {
                                    pane_id,
                                    name,
                                    name_error: Some(Self::DUPLICATE_PANE_NAME_ERROR.to_string()),
                                    agent_index,
                                    focus,
                                });
                                return Ok(());
                            }
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
                            if selected < AGENT_PRESETS.len()
                                && self.agent_available_for_pane(pane_id, selected)
                            {
                                agent_index = selected;
                                focus = PanelSettingsFocus::Agent;
                            }
                        }
                    }

                    self.modal = Some(Modal::PanelSettings {
                        pane_id,
                        name,
                        name_error,
                        agent_index,
                        focus,
                    });
                    return Ok(());
                }
                Modal::WorkspaceSettings {
                    workspace_index,
                    name,
                    name_error,
                    mut cursor,
                    mut action_index,
                } => {
                    let area = workspace_settings_modal_area(size);
                    let name_area = workspace_settings_name_input_area(area);
                    let name_inner = ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .inner(name_area);
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if let Some(selected) =
                            workspace_settings_action_hit_index(area, mouse.column, mouse.row)
                        {
                            action_index = selected;
                        } else if contains(name_inner, mouse.column, mouse.row) {
                            let click_col = mouse.column.saturating_sub(name_inner.x) as usize;
                            cursor = click_col.min(name.chars().count());
                        }
                    }

                    self.modal = Some(Modal::WorkspaceSettings {
                        workspace_index,
                        name,
                        name_error,
                        cursor,
                        action_index,
                    });
                    return Ok(());
                }
            }
        }

        if self.forward_active_pane_mouse_drag(size, &mouse)? {
            return Ok(());
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_workspace_sidebar_click(size, mouse.column, mouse.row)?
        {
            self.text_selection = None;
            return Ok(());
        }

        let top_layout = self.top_bar_layout(size, None);
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && workspace_commander_input_hit(top_layout, mouse.column, mouse.row)
        {
            self.focus_commander_from_sidebar(size);
            self.text_selection = None;
            return Ok(());
        }

        if self.commander_focused
            && workspace_commander_input_hit(top_layout, mouse.column, mouse.row)
        {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.commander_scroll_up();
                    return Ok(());
                }
                MouseEventKind::ScrollDown => {
                    self.commander_scroll_down();
                    return Ok(());
                }
                _ => {}
            }
        }

        let clicked = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        let down_button = match mouse.kind {
            MouseEventKind::Down(button) => Some(button),
            _ => None,
        };
        let Some(placement) = self.placement_at(size, mouse.column, mouse.row) else {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if self.commander_focused {
                        self.commander_scroll_up();
                    } else if let Some(pane) = self.focused_pane_mut() {
                        pane.scroll_up();
                    }
                }
                MouseEventKind::ScrollDown => {
                    if self.commander_focused {
                        self.commander_scroll_down();
                    } else if let Some(pane) = self.focused_pane_mut() {
                        pane.scroll_down();
                    }
                }
                _ => {}
            }
            return Ok(());
        };

        let Some((pane_title, pane_command)) = self
            .panes
            .iter()
            .find(|pane| pane.id == placement.pane_id)
            .map(|pane| (pane.title.clone(), pane.command.clone()))
        else {
            return Ok(());
        };
        let pane_title = pane_chrome_title_label(&pane_title, &pane_command);

        let was_focused = self.focused == placement.pane_id;

        // The common case is a plain click in terminal content. Focus it now
        // and skip the expensive divider/boundary walk entirely.
        let inner = pane_inner_area(placement.area, placement.exposed);
        if let Some(button) = down_button {
            if contains(inner, mouse.column, mouse.row) {
                self.focus_pane(placement.pane_id);
                if button == MouseButton::Left && !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    let Some(cell) = Self::pane_mouse_cell(inner, mouse.column, mouse.row) else {
                        return Ok(());
                    };
                    self.start_text_selection(placement.pane_id, cell);
                } else if !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    if self.send_mouse_button_to_pane(
                        placement.pane_id,
                        inner,
                        PaneMouseEventKind::Down,
                        button,
                        mouse.modifiers,
                        mouse.column,
                        mouse.row,
                    )? {
                        self.drag_pane_mouse = Some(DragPaneMouse {
                            pane_id: placement.pane_id,
                            button,
                        });
                    }
                } else {
                    self.drag_pane_mouse = None;
                }
                return Ok(());
            }
        }

        if clicked {
            self.text_selection = None;
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
            self.close_pane();
            return Ok(());
        }

        if clicked && placement.title_hit(&pane_title, was_focused, mouse.column, mouse.row) {
            self.focus_pane(placement.pane_id);
            if self.panes.len() > 1 {
                self.drag_swap = Some(DragPaneSwap {
                    source_pane_id: placement.pane_id,
                    hovered_pane_id: None,
                    moved: false,
                });
            } else {
                self.open_new_pane_picker(placement.pane_id);
            }
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.pane_is_commander(placement.pane_id) {
                    return Ok(());
                }
                if let Some(pane) = self.pane_mut(placement.pane_id) {
                    let inner = pane_inner_area(placement.area, placement.exposed);
                    let Some((x, y)) = Self::pane_mouse_cell(inner, mouse.column, mouse.row) else {
                        return Ok(());
                    };
                    let before = pane.scrollback;
                    pane.scroll_up();
                    if pane.scrollback == before {
                        let _ = pane.send_mouse_wheel(true, x, y)?;
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.pane_is_commander(placement.pane_id) {
                    return Ok(());
                }
                if let Some(pane) = self.pane_mut(placement.pane_id) {
                    let inner = pane_inner_area(placement.area, placement.exposed);
                    let Some((x, y)) = Self::pane_mouse_cell(inner, mouse.column, mouse.row) else {
                        return Ok(());
                    };
                    let before = pane.scrollback;
                    pane.scroll_down();
                    if pane.scrollback == before {
                        let _ = pane.send_mouse_wheel(false, x, y)?;
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
        self.split_pane_with_agent(pane_id, side, self.default_agent_index, terminal_size)
    }

    pub(crate) fn split_pane_with_agent(
        &mut self,
        pane_id: usize,
        side: SplitSide,
        agent_index: usize,
        terminal_size: Rect,
    ) -> anyhow::Result<usize> {
        let agent_index = self.first_available_agent_for_pane(self.next_pane_id, agent_index);
        let command = AGENT_PRESETS
            .get(agent_index)
            .map(|preset| preset.command)
            .unwrap_or(AGENT_PRESETS[0].command);
        self.split_pane_with_command(pane_id, side, command, terminal_size)
    }

    fn split_pane_with_command(
        &mut self,
        pane_id: usize,
        side: SplitSide,
        command: &str,
        terminal_size: Rect,
    ) -> anyhow::Result<usize> {
        let new_id = self.next_pane_id;
        if !self.command_available_for_pane(new_id, command) {
            anyhow::bail!("selected type is unavailable")
        }
        self.next_pane_id = self.next_pane_id.saturating_add(1);

        let title = format!("Pane {}", new_id + 1);
        self.panes
            .push(Pane::new(new_id, title, command, None, None, 1, 1)?);

        if self.layout.split_leaf(pane_id, side, new_id) {
            self.resize(terminal_size.height, terminal_size.width);
            self.persist_layout();
            Ok(new_id)
        } else {
            self.next_pane_id = self.next_pane_id.saturating_sub(1);
            let _ = self.panes.pop();
            anyhow::bail!("pane not found")
        }
    }

    pub(crate) fn focus_adjacent(&mut self, size: Rect, side: SplitSide) -> bool {
        let placements = self.pane_placements(Self::content_area(size));
        let Some(current_area) = placements
            .iter()
            .find(|placement| placement.pane_id == self.focused)
            .map(|placement| placement.area)
        else {
            return false;
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
            return true;
        }
        false
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
        let default_index = self.first_available_agent_for_pane(new_id, self.default_agent_index);
        self.panes.push(Pane::new(
            new_id,
            title,
            AGENT_PRESETS[default_index].command,
            None,
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
        let cache = self.hit_test_cache.as_ref()?;
        if cache.content != content || !contains(content, x, y) {
            return None;
        }

        let row = y.saturating_sub(content.y) as usize;
        let candidates = cache.row_candidates.get(row)?;
        for &idx in candidates {
            let Some(placement) = cache.placements.get(idx) else {
                continue;
            };
            if contains(placement.area, x, y) {
                return Some(Placement {
                    pane_id: placement.pane_id,
                    area: placement.area,
                    exposed: placement.exposed,
                });
            }
        }
        None
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

    pub(crate) fn pointer_shape_at(&self, size: Rect, x: u16, y: u16) -> MousePointerShape {
        if self.modal.is_some() {
            return MousePointerShape::Default;
        }

        if let Some(drag) = &self.drag_resize {
            return match drag.direction {
                Direction::Horizontal => MousePointerShape::HorizontalResize,
                Direction::Vertical => MousePointerShape::VerticalResize,
            };
        }

        self.resize_target_at(size, x, y)
            .map(|target| match target.direction {
                Direction::Horizontal => MousePointerShape::HorizontalResize,
                Direction::Vertical => MousePointerShape::VerticalResize,
            })
            .unwrap_or(MousePointerShape::Default)
    }

    pub(crate) fn resize_preview_pane_ids(&self) -> Option<&[usize]> {
        self.drag_resize
            .as_ref()
            .map(|drag| drag.pane_ids.as_slice())
    }

    pub(crate) fn pane_swap_preview_target(&self) -> Option<usize> {
        self.drag_swap
            .as_ref()
            .filter(|drag| drag.moved)
            .and_then(|drag| drag.hovered_pane_id)
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

    pub(crate) fn focused_pane_is_commander(&self) -> bool {
        self.commander_focused
    }

    pub(crate) fn pane_is_commander(&self, pane_id: usize) -> bool {
        self.panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| pane.command == COMMANDER_COMMAND)
            .unwrap_or(false)
    }

    pub(crate) fn commander_input(&self) -> &str {
        &self.commander.input
    }

    pub(crate) fn commander_cursor(&self) -> usize {
        self.commander.cursor
    }

    pub(crate) fn commander_busy(&self) -> bool {
        self.commander.busy
    }

    pub(crate) fn commander_history(&self) -> &[String] {
        &self.commander.history
    }

    pub(crate) fn commander_chat_offset_from_bottom(&self) -> usize {
        self.commander.chat_offset_from_bottom
    }

    #[allow(dead_code)]
    pub(crate) fn commander_chat_pinned_to_bottom(&self) -> bool {
        self.commander.chat_pinned_to_bottom
    }

    #[allow(dead_code)]
    pub(crate) fn commander_scroll_to_top(&mut self) {
        let viewport = self.commander.chat_viewport_lines.max(1) as usize;
        let total = self.commander.chat_total_lines;
        self.commander.chat_offset_from_bottom = total.saturating_sub(viewport);
        self.commander.chat_pinned_to_bottom = false;
        self.clamp_commander_chat_scroll();
    }

    #[allow(dead_code)]
    pub(crate) fn commander_scroll_to_bottom(&mut self) {
        self.commander.chat_offset_from_bottom = 0;
        self.commander.chat_pinned_to_bottom = true;
    }

    pub(crate) fn set_commander_chat_metrics(&mut self, viewport_lines: u16, total_lines: usize) {
        self.commander.chat_viewport_lines = viewport_lines;
        self.commander.chat_total_lines = total_lines;
        self.clamp_commander_chat_scroll();
    }

    pub(crate) fn commander_scroll_up(&mut self) {
        self.commander.chat_offset_from_bottom =
            self.commander.chat_offset_from_bottom.saturating_add(1);
        self.commander.chat_pinned_to_bottom = false;
        self.clamp_commander_chat_scroll();
    }

    pub(crate) fn commander_scroll_down(&mut self) {
        if self.commander.chat_offset_from_bottom > 0 {
            self.commander.chat_offset_from_bottom -= 1;
        }
        if self.commander.chat_offset_from_bottom == 0 {
            self.commander.chat_pinned_to_bottom = true;
        }
    }

    pub(crate) fn commander_page_up(&mut self) {
        let step = self.commander.chat_viewport_lines.max(1) as usize;
        self.commander.chat_offset_from_bottom = self
            .commander
            .chat_offset_from_bottom
            .saturating_add(step);
        self.commander.chat_pinned_to_bottom = false;
        self.clamp_commander_chat_scroll();
    }

    pub(crate) fn commander_page_down(&mut self) {
        let step = self.commander.chat_viewport_lines.max(1) as usize;
        self.commander.chat_offset_from_bottom = self
            .commander
            .chat_offset_from_bottom
            .saturating_sub(step);
        if self.commander.chat_offset_from_bottom == 0 {
            self.commander.chat_pinned_to_bottom = true;
        }
    }

    fn clamp_commander_chat_scroll(&mut self) {
        let viewport = self.commander.chat_viewport_lines.max(1) as usize;
        let max_offset = self
            .commander
            .chat_total_lines
            .saturating_sub(viewport);
        if self.commander.chat_offset_from_bottom > max_offset {
            self.commander.chat_offset_from_bottom = max_offset;
        }
    }

    fn commander_sync_scroll_after_history_change(&mut self) {
        if self.commander.chat_pinned_to_bottom {
            self.commander.chat_offset_from_bottom = 0;
        } else {
            self.clamp_commander_chat_scroll();
        }
    }

    pub(crate) fn commander_phase_label(&self) -> &'static str {
        if self.commander.busy {
            return "thinking";
        }
        match self.commander.phase {
            CommanderPhase::Discussing => "planning",
            CommanderPhase::AwaitingApproval => "awaiting /approve",
            CommanderPhase::Executing => "executing",
        }
    }

    fn commander_submit_current_input(&mut self) {
        let user_input = self.commander.input.trim().to_string();
        if user_input.is_empty() || self.commander.busy {
            return;
        }
        self.start_commander_palette_video();

        if self.handle_commander_slash_command(&user_input) {
            self.commander.input.clear();
            self.commander.cursor = 0;
            return;
        }

        if self.commander.phase == CommanderPhase::AwaitingApproval && is_plan_approval(&user_input)
        {
            self.commander.input.clear();
            self.commander.cursor = 0;
            self.commander
                .history
                .push(format!("You: {}", user_input.clone()));
            self.commander_sync_scroll_after_history_change();
            self.execute_approved_commander_plan();
            return;
        }

        if self.commander.phase == CommanderPhase::AwaitingApproval
            && is_plan_revision_request(&user_input)
        {
            self.commander.phase = CommanderPhase::Discussing;
            self.commander.pending_plan = None;
        }

        self.commander
            .history
            .push(format!("You: {}", user_input.clone()));
        self.commander
            .history
            .push("Commander: thinking...".to_string());
        self.commander_sync_scroll_after_history_change();

        self.commander.input.clear();
        self.commander.cursor = 0;
        self.commander.busy = true;

        let pane_names = self
            .panes
            .iter()
            .filter(|pane| pane.command != COMMANDER_COMMAND)
            .map(|pane| pane.title.clone())
            .collect::<Vec<_>>();
        let pane_roster = self
            .panes
            .iter()
            .map(|pane| {
                let mut status = Vec::new();
                if pane.id == self.focused {
                    status.push("focused");
                }
                if pane.command == COMMANDER_COMMAND {
                    status.push("commander");
                } else {
                    status.push("worker");
                }
                if pane.exited {
                    status.push("exited");
                } else {
                    status.push("running");
                }
                if pane.relaunch_failed {
                    status.push("relaunch_failed");
                }
                format!(
                    "- id:{} | name:{} | type:{} | status:{}{}",
                    pane.id,
                    pane.title,
                    pane.command,
                    status.join(","),
                    self.agent_handoffs
                        .get(&pane.id)
                        .map(|handoff| format!(" | handoff:{}", format_agent_handoff(handoff)))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        let agent_types = AGENT_PRESETS
            .iter()
            .filter(|preset| preset.command != COMMANDER_COMMAND)
            .map(|preset| preset.command.to_string())
            .collect::<Vec<_>>();
        let workspace_roster = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(idx, workspace)| {
                let status = if idx == self.active_workspace {
                    "active"
                } else {
                    "inactive"
                };
                format!(
                    "- {} | name:{} | status:{}",
                    idx + 1,
                    workspace.name,
                    status
                )
            })
            .collect::<Vec<_>>();
        let phase = self.commander.phase;
        let pending_plan = self.commander.pending_plan.clone();
        let conversation = self
            .commander
            .history
            .iter()
            .filter(|line| *line != "Commander: thinking...")
            .cloned()
            .collect::<Vec<_>>();
        let (tx, rx) = mpsc::channel();
        self.commander.rx = Some(rx);

        thread::spawn(move || {
            let result = run_commander_harness(
                user_input,
                phase,
                conversation,
                pending_plan,
                pane_names,
                pane_roster,
                agent_types,
                workspace_roster,
            );
            let _ = tx.send(result);
        });
    }

    fn execute_approved_commander_plan(&mut self) {
        let Some(plan) = self.commander.pending_plan.clone() else {
            self.commander.history.push(
                "Commander: No plan is waiting for approval. Describe the task first.".to_string(),
            );
            self.commander_sync_scroll_after_history_change();
            self.queue_tts("No plan is waiting for approval.");
            return;
        };

        self.commander.busy = true;
        self.commander.phase = CommanderPhase::Executing;
        self.commander
            .history
            .push("Commander: executing the approved plan...".to_string());

        let steps = self.build_execution_steps_from_plan(&plan);
        let (_history_notes, speech_notes) = self.execute_commander_steps(steps);
        self.commander.busy = false;
        self.commander.phase = CommanderPhase::Discussing;
        self.commander.pending_plan = None;

        let commander_reply = if speech_notes.is_empty() {
            format!("Started execution for: {}.", plan.summary)
        } else {
            format_commander_execution_reply(&speech_notes)
        };
        if let Some(last) = self.commander.history.last_mut() {
            if last == "Commander: executing the approved plan..." {
                *last = format!("Commander: {}", commander_reply);
            } else {
                self.commander
                    .history
                    .push(format!("Commander: {}", commander_reply));
            }
        }
        self.commander_sync_scroll_after_history_change();
        self.queue_tts(&commander_reply);
    }

    fn handle_commander_slash_command(&mut self, input: &str) -> bool {
        let Some(command) = parse_commander_slash_command(input) else {
            return false;
        };

        self.commander
            .history
            .push(format!("You: {}", input.trim()));

        match command {
            CommanderSlashCommand::ApprovePlan => {
                self.execute_approved_commander_plan();
            }
            CommanderSlashCommand::CancelPlan => {
                self.commander.pending_plan = None;
                self.commander.phase = CommanderPhase::Discussing;
                let note = "Plan cleared. We can keep refining the idea.";
                self.commander.history.push(format!("Commander: {}", note));
                self.queue_tts(note);
            }
            CommanderSlashCommand::OpenTheme => {
                self.modal = Some(Modal::Theme);
                self.commander
                    .history
                    .push("Commander: opening the theme picker.".to_string());
                self.queue_tts("Opening the theme picker.");
            }
            CommanderSlashCommand::OpenSettings => {
                self.modal = Some(Modal::Help);
                self.commander
                    .history
                    .push("Commander: opening settings.".to_string());
                self.queue_tts("Opening settings.");
            }
            CommanderSlashCommand::CreateWorkspace(name) => {
                let note = self
                    .create_workspace_from_request(name.as_deref())
                    .unwrap_or_else(|err| err);
                self.commander.history.push(format!("Commander: {}", note));
                self.queue_tts(&note);
            }
            CommanderSlashCommand::SwitchWorkspace(selector) => {
                let note = self
                    .switch_workspace_from_request(&selector)
                    .unwrap_or_else(|err| err);
                self.commander.history.push(format!("Commander: {}", note));
                self.queue_tts(&note);
            }
        }

        self.commander_sync_scroll_after_history_change();
        true
    }

    fn poll_commander_result(&mut self) -> bool {
        let Some(rx) = self.commander.rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.commander.rx = None;
                self.commander.busy = false;

                if let Some(plan) = result.proposed_plan.clone() {
                    self.commander.pending_plan = Some(plan);
                    self.commander.phase = CommanderPhase::AwaitingApproval;
                } else if result.execution_allowed {
                    self.commander.pending_plan = None;
                    if self.commander.phase == CommanderPhase::AwaitingApproval {
                        self.commander.phase = CommanderPhase::Discussing;
                    }
                }

                let mut commander_reply = result
                    .reply_text
                    .clone()
                    .or_else(|| result.speech_text.clone())
                    .unwrap_or_else(|| "Done.".to_string());

                if result.execution_allowed {
                    let steps = result.into_steps();
                    if !steps.is_empty() {
                        let (_executed_notes, speech_notes) = self.execute_commander_steps(steps);
                        if !speech_notes.is_empty() {
                            commander_reply = format_commander_execution_reply(&speech_notes);
                        }
                    }
                }

                if self.commander.phase == CommanderPhase::AwaitingApproval {
                    commander_reply = format!(
                        "{commander_reply}\n\nReply /approve when you want me to run this plan."
                    );
                }

                self.queue_tts(&commander_reply);
                if let Some(last) = self.commander.history.last_mut() {
                    if last == "Commander: thinking..." {
                        *last = format!("Commander: {}", commander_reply);
                    } else {
                        self.commander
                            .history
                            .push(format!("Commander: {}", commander_reply));
                    }
                } else {
                    self.commander
                        .history
                        .push(format!("Commander: {}", commander_reply));
                }
                self.commander_sync_scroll_after_history_change();
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.commander.rx = None;
                self.commander.busy = false;
                let error_line = "failed to read LLM response (worker disconnected).";
                self.commander
                    .history
                    .push(format!("Commander: {}", error_line));
                self.commander_sync_scroll_after_history_change();
                self.queue_tts(error_line);
                true
            }
        }
    }

    fn workspace_has_dedicated_workers(&self) -> bool {
        self.panes
            .iter()
            .any(|pane| pane.command != COMMANDER_COMMAND)
    }

    fn build_execution_steps_from_plan(
        &self,
        plan: &CommanderPendingPlan,
    ) -> Vec<CommanderExecutionStep> {
        let mut steps = Vec::new();
        let workspace = match &plan.workspace {
            CommanderWorkspacePlan::UseCurrent if !self.workspace_has_dedicated_workers() => {
                CommanderWorkspacePlan::CreateNew
            }
            other => other.clone(),
        };

        match workspace {
            CommanderWorkspacePlan::CreateNew => {
                steps.push(CommanderExecutionStep {
                    workspace_create: Some(plan.workspace_name.clone()),
                    ..CommanderExecutionStep::default()
                });
            }
            CommanderWorkspacePlan::SwitchTo(selector) => {
                steps.push(CommanderExecutionStep {
                    workspace_switch: Some(selector),
                    ..CommanderExecutionStep::default()
                });
            }
            CommanderWorkspacePlan::UseCurrent => {}
        }

        for assignment in &plan.panes {
            if assignment.task.trim().is_empty() {
                continue;
            }
            steps.push(CommanderExecutionStep {
                create_requests: vec![(assignment.agent.clone(), 1)],
                rename_requests: vec![("ref:last".to_string(), assignment.pane_name.clone())],
                target_name: Some("ref:last".to_string()),
                payload: Some(assignment.task.clone()),
                submit_payload: true,
                speech_text: Some(format!(
                    "Tasked {} with {}.",
                    assignment.pane_name, assignment.agent
                )),
                ..CommanderExecutionStep::default()
            });
        }

        if steps.is_empty() {
            steps.push(CommanderExecutionStep {
                reply_text: Some("The approved plan had no executable pane tasks.".to_string()),
                ..CommanderExecutionStep::default()
            });
        }

        steps
    }

    fn execute_commander_steps(
        &mut self,
        steps: Vec<CommanderExecutionStep>,
    ) -> (Vec<String>, Vec<String>) {
        let mut history_notes = Vec::new();
        let mut speech_notes = Vec::new();
        let mut refs = CommanderStepRefs::default();

        for step in steps {
            let step_result = self.execute_commander_step(step, &mut refs);
            history_notes.extend(step_result.history_notes);
            speech_notes.extend(step_result.speech_notes);
        }

        (history_notes, speech_notes)
    }

    fn execute_commander_step(
        &mut self,
        step: CommanderExecutionStep,
        refs: &mut CommanderStepRefs,
    ) -> CommanderStepOutcome {
        let should_refocus_commander = !step.create_requests.is_empty()
            || !step.rename_requests.is_empty()
            || !step.close_requests.is_empty();
        let mut history_notes = Vec::new();
        let mut speech_notes = Vec::new();
        let reply_note = step
            .reply_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("none"));
        let creation = self.create_panes_from_requests(&step.create_requests);
        if !creation.created_ids.is_empty() {
            refs.last_created_ids = creation.created_ids.clone();
        }

        if let Some(save_as) = step
            .save_as
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("none"))
        {
            refs.named
                .insert(save_as.to_ascii_lowercase(), creation.created_ids.clone());
        }

        let rename_requests = step
            .rename_requests
            .iter()
            .flat_map(|(selector, name)| {
                self.expand_selector_for_requests(selector, refs, true)
                    .into_iter()
                    .map(move |expanded| (expanded, name.clone()))
            })
            .collect::<Vec<_>>();
        let rename_note = self.rename_panes_from_requests(&rename_requests, &creation.created_ids);

        let delivery_note = if let (Some(target_name), Some(payload)) =
            (step.target_name.as_deref(), step.payload.as_deref())
        {
            match self.dispatch_to_target_selector(
                target_name,
                payload,
                step.submit_payload,
                &creation.created_ids,
                refs,
            ) {
                Ok(summary) => Some(summary),
                Err(err) => Some(CommanderExecutionSummary { history_note: err }),
            }
        } else {
            None
        };

        let close_requests = step
            .close_requests
            .iter()
            .flat_map(|selector| self.expand_selector_for_requests(selector, refs, false))
            .collect::<Vec<_>>();
        let close_note = self.close_panes_from_requests(&close_requests);

        let workspace_create_note = if let Some(name) = step.workspace_create.as_ref() {
            Some(
                self.create_workspace_from_request(name.as_deref())
                    .unwrap_or_else(|err| err),
            )
        } else {
            None
        };

        let workspace_note = step.workspace_switch.as_deref().and_then(|selector| {
            match self.switch_workspace_from_request(selector) {
                Ok(note) => Some(note),
                Err(err) => Some(err),
            }
        });

        if should_refocus_commander {
            self.focus_commander_pane();
        }

        for note in [
            reply_note,
            creation.note.as_deref(),
            rename_note.as_deref(),
            close_note.as_deref(),
            workspace_create_note.as_deref(),
            workspace_note.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let note = note.to_string();
            history_notes.push(note.clone());
        }

        if let Some(summary) = delivery_note {
            history_notes.push(summary.history_note);
        }

        if let Some(speech_text) = step
            .speech_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("none"))
        {
            speech_notes.push(speech_text.to_string());
        } else if !history_notes.is_empty() {
            speech_notes.push(format_commander_execution_reply(&history_notes));
        }

        CommanderStepOutcome {
            history_notes,
            speech_notes,
        }
    }

    fn expand_selector_for_requests(
        &self,
        selector: &str,
        refs: &CommanderStepRefs,
        filter_non_commander: bool,
    ) -> Vec<String> {
        if let Some(ids) = resolve_reference_selector(selector, refs) {
            let filtered = ids
                .into_iter()
                .filter(|id| {
                    self.panes.iter().any(|pane| {
                        pane.id == *id
                            && (!filter_non_commander || pane.command != COMMANDER_COMMAND)
                    })
                })
                .collect::<Vec<_>>();
            if filtered.is_empty() {
                return vec![selector.to_string()];
            }
            return filtered
                .into_iter()
                .map(|id| format!("id:{id}"))
                .collect::<Vec<_>>();
        }
        vec![selector.to_string()]
    }

    fn dispatch_to_target_selector(
        &mut self,
        target_selector: &str,
        payload: &str,
        submit_payload: bool,
        newly_created_ids: &[usize],
        refs: &CommanderStepRefs,
    ) -> Result<CommanderExecutionSummary, String> {
        let selectors = target_selector
            .split(',')
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .collect::<Vec<_>>();

        let mut pane_ids = if selectors.is_empty() {
            Vec::new()
        } else {
            selectors
                .into_iter()
                .flat_map(|selector| {
                    self.resolve_send_selector_with_refs(selector, newly_created_ids, refs)
                })
                .collect::<Vec<_>>()
        };
        pane_ids.sort_unstable();
        pane_ids.dedup();
        if pane_ids.is_empty() {
            return Err(format!(
                "Couldn't find a pane matching {}.",
                target_selector
            ));
        }

        let press_enter = true;

        // Give newly created panes time to initialize before delivery.
        let targets_new_panes = pane_ids.iter().any(|id| newly_created_ids.contains(id));
        if targets_new_panes {
            let startup_wait = if submit_payload || press_enter {
                Duration::from_millis(2800)
            } else {
                Duration::from_millis(1600)
            };
            thread::sleep(startup_wait);
        }

        let pane_count = pane_ids.len();
        let mut sent = 0usize;
        let mut failed = 0usize;

        for (idx, pane_id) in pane_ids.iter().copied().enumerate() {
            if self.panes.iter().all(|pane| pane.id != pane_id) {
                failed += 1;
                continue;
            }

            let Some((pane_title, pane_command)) = self
                .panes
                .iter()
                .find(|pane| pane.id == pane_id)
                .map(|pane| (pane.title.clone(), pane.command.clone()))
            else {
                failed += 1;
                continue;
            };

            let outgoing = build_outgoing_payload(
                payload,
                submit_payload,
                pane_count,
                idx + 1,
                &pane_title,
                &pane_command,
            );

            let delivered = self.deliver_payload_with_retries(
                pane_id,
                &outgoing,
                press_enter,
                newly_created_ids.contains(&pane_id),
            );
            if !delivered {
                failed += 1;
                continue;
            }
            sent += 1;
        }

        if sent == 0 {
            return Err(if submit_payload {
                "Couldn't send or start that task.".to_string()
            } else {
                "Couldn't send that.".to_string()
            });
        }

        let sent_phrase = count_phrase(sent, "pane", "panes");
        let failed_note = if failed > 0 {
            format!(
                " Couldn't deliver to {}.",
                count_phrase(failed, "pane", "panes")
            )
        } else {
            String::new()
        };

        let history_note = if submit_payload && pane_count > 1 {
            format!("Started this task in {}.{}", sent_phrase, failed_note)
        } else if submit_payload {
            format!("Started this task in {}.{}", sent_phrase, failed_note)
        } else {
            format!("Sent that to {}.{}", sent_phrase, failed_note)
        };

        Ok(CommanderExecutionSummary { history_note })
    }

    fn deliver_payload_with_retries(
        &mut self,
        pane_id: usize,
        outgoing: &str,
        press_enter: bool,
        newly_created: bool,
    ) -> bool {
        let attempts = if newly_created { 8 } else { 3 };
        let retry_delay = if newly_created {
            Duration::from_millis(350)
        } else {
            Duration::from_millis(120)
        };

        let mut paste_sent = false;
        for attempt in 0..attempts {
            let Some(pane) = self.pane_mut(pane_id) else {
                return false;
            };

            if !paste_sent {
                if pane.send_paste(outgoing).is_ok() {
                    paste_sent = true;
                } else {
                    if attempt + 1 < attempts {
                        thread::sleep(retry_delay);
                    }
                    continue;
                }
            }

            if !press_enter {
                return true;
            }

            // Some terminal apps debounce input right after large pastes.
            thread::sleep(Duration::from_millis(180));
            if pane.send(&[b'\r']).is_ok() {
                return true;
            }

            if attempt + 1 < attempts {
                thread::sleep(retry_delay);
            }
        }

        false
    }

    fn resolve_send_selector(&self, selector: &str, newly_created_ids: &[usize]) -> Vec<usize> {
        let raw = selector.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
            return Vec::new();
        }
        let normalized = raw.to_ascii_lowercase();

        if matches!(normalized.as_str(), "new" | "newest" | "last_created") {
            return newly_created_ids.to_vec();
        }
        if normalized == "all" {
            return self
                .panes
                .iter()
                .filter(|pane| pane.command != COMMANDER_COMMAND)
                .map(|pane| pane.id)
                .collect();
        }
        if normalized == "focused" {
            return self
                .panes
                .iter()
                .filter(|pane| pane.id == self.focused && pane.command != COMMANDER_COMMAND)
                .map(|pane| pane.id)
                .collect();
        }
        if let Some(id_str) = normalized.strip_prefix("id:") {
            if let Ok(id) = id_str.trim().parse::<usize>() {
                return self
                    .panes
                    .iter()
                    .filter(|pane| pane.id == id && pane.command != COMMANDER_COMMAND)
                    .map(|pane| pane.id)
                    .collect();
            }
        }
        if let Some(status) = normalized.strip_prefix("status:") {
            if status.trim() == "new" {
                return newly_created_ids.to_vec();
            }
            return self
                .panes
                .iter()
                .filter(|pane| {
                    pane.command != COMMANDER_COMMAND
                        && pane_matches_status(pane, status.trim(), self.focused)
                })
                .map(|pane| pane.id)
                .collect();
        }
        if let Some(command) = normalized.strip_prefix("type:") {
            let command = command.trim();
            return self
                .panes
                .iter()
                .filter(|pane| {
                    pane.command != COMMANDER_COMMAND && pane.command.eq_ignore_ascii_case(command)
                })
                .map(|pane| pane.id)
                .collect();
        }
        if let Some(name) = normalized.strip_prefix("name:") {
            let name = name.trim();
            return self
                .panes
                .iter()
                .filter(|pane| {
                    pane.command != COMMANDER_COMMAND && pane.title.to_ascii_lowercase() == name
                })
                .map(|pane| pane.id)
                .collect();
        }
        if let Some(n) = normalized
            .strip_prefix("pane ")
            .and_then(|n| n.trim().parse::<usize>().ok())
        {
            let pane_title = format!("pane {}", n);
            let mut matches = self
                .panes
                .iter()
                .filter(|pane| {
                    pane.command != COMMANDER_COMMAND
                        && pane.title.to_ascii_lowercase() == pane_title
                })
                .map(|pane| pane.id)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                matches = self
                    .panes
                    .iter()
                    .filter(|pane| {
                        pane.command != COMMANDER_COMMAND && pane.id == n.saturating_sub(1)
                    })
                    .map(|pane| pane.id)
                    .collect();
            }
            return matches;
        }

        let status_matches = self
            .panes
            .iter()
            .filter(|pane| {
                pane.command != COMMANDER_COMMAND
                    && pane_matches_status(pane, &normalized, self.focused)
            })
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        if !status_matches.is_empty() {
            return status_matches;
        }

        let exact_name_matches = self
            .panes
            .iter()
            .filter(|pane| {
                pane.command != COMMANDER_COMMAND && pane.title.to_ascii_lowercase() == normalized
            })
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        if !exact_name_matches.is_empty() {
            return exact_name_matches;
        }

        self.panes
            .iter()
            .filter(|pane| {
                pane.command != COMMANDER_COMMAND
                    && (pane.title.to_ascii_lowercase().contains(&normalized)
                        || pane.command.to_ascii_lowercase().contains(&normalized))
            })
            .map(|pane| pane.id)
            .collect()
    }

    fn resolve_send_selector_with_refs(
        &self,
        selector: &str,
        newly_created_ids: &[usize],
        refs: &CommanderStepRefs,
    ) -> Vec<usize> {
        if let Some(ids) = resolve_reference_selector(selector, refs) {
            return ids
                .into_iter()
                .filter(|id| {
                    self.panes
                        .iter()
                        .any(|pane| pane.id == *id && pane.command != COMMANDER_COMMAND)
                })
                .collect();
        }
        self.resolve_send_selector(selector, newly_created_ids)
    }

    fn create_panes_from_requests(&mut self, requests: &[(String, usize)]) -> CreateOutcome {
        if requests.is_empty() {
            return CreateOutcome {
                note: None,
                created_ids: Vec::new(),
            };
        }

        let mut created = Vec::new();
        let mut created_ids = Vec::new();
        let mut failures = Vec::new();

        for (agent, count) in requests {
            let Some(agent_index) = agent_index_for_alias(agent) else {
                let _ = agent;
                failures
                    .push("Couldn't create a pane because that type isn't available.".to_string());
                continue;
            };
            let command = AGENT_PRESETS[agent_index].command;
            for _ in 0..*count {
                let Some((target_pane, side)) = self.largest_split_target() else {
                    failures
                        .push("Couldn't create a pane because nothing could be split.".to_string());
                    break;
                };
                match self.split_pane_with_command(
                    target_pane,
                    side,
                    command,
                    self.last_terminal_size,
                ) {
                    Ok(new_id) => {
                        self.focus_pane(new_id);
                        created.push(command.to_string());
                        created_ids.push(new_id);
                    }
                    Err(_) => {
                        failures.push("Couldn't create a pane because split failed.".to_string());
                        break;
                    }
                }
            }
        }

        if !created.is_empty() {
            self.persist_layout();
            self.resize(
                self.last_terminal_size.height,
                self.last_terminal_size.width,
            );
        }

        let mut notes = Vec::new();
        if !created.is_empty() {
            notes.push(format!("Created {}.", summarize_created_panes(&created)));
        }
        if !failures.is_empty() {
            notes.push(failures.join(" "));
        }
        CreateOutcome {
            note: Some(notes.join(" ")),
            created_ids,
        }
    }

    fn rename_panes_from_requests(
        &mut self,
        requests: &[(String, String)],
        newly_created_ids: &[usize],
    ) -> Option<String> {
        if requests.is_empty() {
            return None;
        }

        let mut renamed = HashSet::new();
        let mut unchanged = HashSet::new();
        let mut unmatched = 0usize;
        let mut invalid = 0usize;
        let mut duplicate_name = 0usize;

        for (selector, new_name_raw) in requests {
            let new_name = new_name_raw.trim();
            if new_name.is_empty() {
                invalid += 1;
                continue;
            }

            let mut matches = self.resolve_send_selector(selector, newly_created_ids);
            matches.sort_unstable();
            matches.dedup();
            if matches.is_empty() {
                unmatched += 1;
                continue;
            }

            for pane_id in matches {
                let Some(current_title) = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == pane_id)
                    .map(|pane| pane.title.clone())
                else {
                    continue;
                };
                if current_title == new_name {
                    if !renamed.contains(&pane_id) {
                        unchanged.insert(pane_id);
                    }
                    continue;
                }
                if self.pane_name_exists_for_other(pane_id, new_name) {
                    duplicate_name += 1;
                    continue;
                }
                let Some(pane) = self.pane_mut(pane_id) else {
                    continue;
                };
                pane.title = new_name.to_string();
                unchanged.remove(&pane_id);
                renamed.insert(pane_id);
            }
        }

        if !renamed.is_empty() {
            self.persist_layout();
        }

        let mut notes = Vec::new();
        if !renamed.is_empty() {
            notes.push(format!(
                "Renamed {}.",
                count_phrase(renamed.len(), "pane", "panes")
            ));
        }
        if !unchanged.is_empty() {
            notes.push(format!(
                "{} already had that name.",
                count_phrase(unchanged.len(), "pane", "panes")
            ));
        }
        if unmatched > 0 {
            notes.push("Couldn't rename some panes because nothing matched.".to_string());
        }
        if invalid > 0 {
            notes.push("Skipped some rename requests because the new name was empty.".to_string());
        }
        if duplicate_name > 0 {
            notes.push("Skipped some rename requests because the name already exists.".to_string());
        }
        if notes.is_empty() {
            notes.push("No panes renamed.".to_string());
        }
        Some(notes.join(" "))
    }

    fn queue_tts(&self, text: &str) {
        if let Some(tx) = self.tts.tx.as_ref() {
            let _ = tx.send(text.trim().to_string());
        }
    }

    pub(crate) fn commander_palette_video_frame(&self) -> Option<&str> {
        if self.commander_palette_video.child.is_some()
            && !self.commander_palette_video.frame_text.trim().is_empty()
        {
            Some(self.commander_palette_video.frame_text.as_str())
        } else {
            None
        }
    }

    pub(crate) fn set_commander_palette_video_size(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.commander_palette_video.rows && cols == self.commander_palette_video.cols {
            return;
        }
        self.commander_palette_video.rows = rows;
        self.commander_palette_video.cols = cols;
        self.commander_palette_video
            .parser
            .set_size(commander_palette_video_source_rows(rows), cols);
        self.commander_palette_video.frame_text.clear();
        if self.commander_palette_video.child.is_some() {
            self.start_commander_palette_video();
        }
    }

    fn poll_tts_events(&mut self) -> bool {
        let Some(rx) = self.tts.event_rx.as_ref() else {
            return false;
        };
        let mut saw_event = false;
        let mut saw_speak_finished = false;
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(TtsEvent::SpeakStarted) => {
                    saw_event = true;
                }
                Ok(TtsEvent::SpeakFinished) => {
                    saw_event = true;
                    saw_speak_finished = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if saw_speak_finished {
            self.stop_commander_palette_video();
        }
        if disconnected {
            self.tts.event_rx = None;
        }
        saw_event
    }

    fn poll_commander_palette_video(&mut self) -> bool {
        let Some(rx) = self.commander_palette_video.rx.as_ref() else {
            return false;
        };
        let mut updated = false;
        loop {
            match rx.try_recv() {
                Ok(bytes) => {
                    self.commander_palette_video.parser.process(&bytes);
                    let rows: Vec<String> = {
                        let screen = self.commander_palette_video.parser.screen();
                        let (_, cols) = screen.size();
                        screen.rows(0, cols).collect()
                    };
                    self.commander_palette_video.frame_text = normalize_commander_video_frame(
                        rows,
                        self.commander_palette_video.rows,
                        self.commander_palette_video.cols,
                    );
                    updated = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.commander_palette_video.rx = None;
                    break;
                }
            }
        }
        updated
    }

    fn start_commander_palette_video(&mut self) {
        if self.tts.tx.is_none() {
            return;
        }
        self.stop_commander_palette_video();

        let bin_path = resolve_ascii_video_binary();
        let video_path = Path::new("/home/aaron/lab/ascii-video/src/video/slash-dance.mp4");
        let Some(bin_path) = bin_path else {
            return;
        };
        if !video_path.exists() {
            return;
        }

        let rows = self.commander_palette_video.rows.max(1);
        let cols = self.commander_palette_video.cols.max(1);
        // ascii-video reserves one terminal row for its status footer.
        // Give it one extra PTY row so the visual content still fills
        // the Commander panel while the footer lands off-screen.
        let source_rows = commander_palette_video_source_rows(rows);
        let command = format!(
            "stty rows {source_rows} cols {cols}; COLUMNS={cols} LINES={source_rows} {} {}",
            sh_quote(bin_path),
            sh_quote(video_path)
        );
        let mut child = match Command::new("script")
            .arg("-qfc")
            .arg(command)
            .arg("/dev/null")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return;
        };

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || pump_commander_video_output(stdout, tx));

        self.commander_palette_video.parser = vt100::Parser::new(source_rows, cols, 0);
        self.commander_palette_video.frame_text.clear();
        self.commander_palette_video.rx = Some(rx);
        self.commander_palette_video.child = Some(child);
    }

    fn stop_commander_palette_video(&mut self) {
        if let Some(mut child) = self.commander_palette_video.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.commander_palette_video.rx = None;
        self.commander_palette_video.frame_text.clear();
    }

    fn close_panes_from_requests(&mut self, requests: &[String]) -> Option<String> {
        if requests.is_empty() {
            return None;
        }

        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        let mut unmatched = Vec::new();

        for selector in requests {
            let matches = self.resolve_close_selector(selector);
            if matches.is_empty() {
                unmatched.push(selector.clone());
                continue;
            }
            for pane_id in matches {
                if seen.insert(pane_id) {
                    targets.push(pane_id);
                }
            }
        }

        let mut closed = Vec::new();
        let mut skipped = Vec::new();

        for pane_id in targets {
            if self.panes.len() <= 1 {
                skipped.push("Skipped one close request to keep the last pane open.".to_string());
                break;
            }
            let Some(_pane) = self.panes.iter().find(|pane| pane.id == pane_id) else {
                skipped.push("Skipped one close request because that pane is gone.".to_string());
                continue;
            };
            self.focus_pane(pane_id);
            self.close_pane();
            closed.push(pane_id);
        }

        let mut notes = Vec::new();
        if !closed.is_empty() {
            notes.push(format!(
                "Closed {}.",
                count_phrase(closed.len(), "pane", "panes")
            ));
        }
        if !unmatched.is_empty() {
            notes.push("Couldn't close some panes because nothing matched.".to_string());
        }
        if !skipped.is_empty() {
            notes.push(skipped.join(" "));
        }
        if notes.is_empty() {
            notes.push("No panes closed.".to_string());
        }
        Some(notes.join(" "))
    }

    fn resolve_close_selector(&self, selector: &str) -> Vec<usize> {
        let raw = selector.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
            return Vec::new();
        }

        let normalized = raw.to_ascii_lowercase();
        if normalized == "all" {
            return self.panes.iter().map(|pane| pane.id).collect();
        }
        if normalized == "focused" {
            return self
                .panes
                .iter()
                .filter(|pane| pane.id == self.focused)
                .map(|pane| pane.id)
                .collect();
        }

        if let Some(id_str) = normalized.strip_prefix("id:") {
            if let Ok(id) = id_str.trim().parse::<usize>() {
                return self
                    .panes
                    .iter()
                    .filter(|pane| pane.id == id)
                    .map(|pane| pane.id)
                    .collect();
            }
        }

        if let Some(status) = normalized.strip_prefix("status:") {
            return self
                .panes
                .iter()
                .filter(|pane| pane_matches_status(pane, status.trim(), self.focused))
                .map(|pane| pane.id)
                .collect();
        }

        if let Some(command) = normalized.strip_prefix("type:") {
            let command = command.trim();
            return self
                .panes
                .iter()
                .filter(|pane| pane.command.eq_ignore_ascii_case(command))
                .map(|pane| pane.id)
                .collect();
        }

        if let Some(name) = normalized.strip_prefix("name:") {
            let name = name.trim();
            return self
                .panes
                .iter()
                .filter(|pane| pane.title.to_ascii_lowercase() == name)
                .map(|pane| pane.id)
                .collect();
        }

        if let Some(n) = normalized
            .strip_prefix("pane ")
            .and_then(|n| n.trim().parse::<usize>().ok())
        {
            let pane_title = format!("pane {}", n);
            let mut matches = self
                .panes
                .iter()
                .filter(|pane| pane.title.to_ascii_lowercase() == pane_title)
                .map(|pane| pane.id)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                matches = self
                    .panes
                    .iter()
                    .filter(|pane| pane.id == n.saturating_sub(1))
                    .map(|pane| pane.id)
                    .collect();
            }
            return matches;
        }

        let status_matches = self
            .panes
            .iter()
            .filter(|pane| pane_matches_status(pane, &normalized, self.focused))
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        if !status_matches.is_empty() {
            return status_matches;
        }

        let exact_name_matches = self
            .panes
            .iter()
            .filter(|pane| pane.title.to_ascii_lowercase() == normalized)
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        if !exact_name_matches.is_empty() {
            return exact_name_matches;
        }

        self.panes
            .iter()
            .filter(|pane| {
                pane.title.to_ascii_lowercase().contains(&normalized)
                    || pane.command.to_ascii_lowercase().contains(&normalized)
            })
            .map(|pane| pane.id)
            .collect()
    }

    fn largest_split_target(&self) -> Option<(usize, SplitSide)> {
        let placements = self.pane_placements(Self::content_area(self.last_terminal_size));
        let best = placements
            .iter()
            .filter(|placement| !self.pane_is_commander(placement.pane_id))
            .max_by_key(|placement| {
                u32::from(placement.area.width) * u32::from(placement.area.height)
            })
            .or_else(|| {
                placements.iter().max_by_key(|placement| {
                    u32::from(placement.area.width) * u32::from(placement.area.height)
                })
            })?;
        let side = if best.area.width >= best.area.height {
            SplitSide::Right
        } else {
            SplitSide::Bottom
        };
        Some((best.pane_id, side))
    }

    pub(crate) fn agent_available_for_pane(&self, pane_id: usize, agent_index: usize) -> bool {
        let command = AGENT_PRESETS
            .get(agent_index)
            .map(|preset| preset.command)
            .unwrap_or(AGENT_PRESETS[0].command);
        self.command_available_for_pane(pane_id, command)
    }

    pub(crate) fn agent_availability_for_pane(&self, pane_id: usize) -> Vec<bool> {
        (0..AGENT_PRESETS.len())
            .map(|idx| self.agent_available_for_pane(pane_id, idx))
            .collect()
    }

    fn command_available_for_pane(&self, pane_id: usize, command: &str) -> bool {
        if command != COMMANDER_COMMAND {
            return true;
        }
        !self
            .panes
            .iter()
            .any(|pane| pane.id != pane_id && pane.command == COMMANDER_COMMAND)
    }

    fn first_available_agent_for_pane(&self, pane_id: usize, preferred: usize) -> usize {
        let preferred = preferred.min(AGENT_PRESETS.len().saturating_sub(1));
        if self.agent_available_for_pane(pane_id, preferred) {
            return preferred;
        }
        (0..AGENT_PRESETS.len())
            .find(|idx| self.agent_available_for_pane(pane_id, *idx))
            .unwrap_or(0)
    }

    fn cycle_available_agent_for_pane(&self, pane_id: usize, current: usize, step: isize) -> usize {
        if AGENT_PRESETS.is_empty() {
            return 0;
        }
        let len = AGENT_PRESETS.len() as isize;
        let mut idx = current.min(AGENT_PRESETS.len().saturating_sub(1)) as isize;
        for _ in 0..AGENT_PRESETS.len() {
            idx = (idx + step).rem_euclid(len);
            let next = idx as usize;
            if self.agent_available_for_pane(pane_id, next) {
                return next;
            }
        }
        current.min(AGENT_PRESETS.len().saturating_sub(1))
    }

    fn focus_pane(&mut self, pane_id: usize) {
        self.commander_focused = false;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.drag_pane_mouse = None;
        self.focused = pane_id;
        if self.maximized_pane.is_some() {
            self.maximized_pane = Some(pane_id);
        }
    }

    fn focus_commander_pane(&mut self) {
        self.commander_focused = true;
        self.sidebar_workspace_focused = None;
        self.sidebar_add_button_focused = false;
        self.drag_pane_mouse = None;
    }

    pub(crate) fn toggle_maximize(&mut self) {
        if self.maximized_pane.is_some() {
            self.maximized_pane = None;
        } else {
            self.maximized_pane = Some(self.focused);
        }
        self.rebuild_hit_test_cache();
    }

    pub(crate) fn is_maximized(&self) -> bool {
        self.maximized_pane.is_some()
    }

    pub(crate) fn pane_mut(&mut self, pane_id: usize) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    pub(crate) fn pane(&self, pane_id: usize) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    fn resize_target_at(&self, size: Rect, x: u16, y: u16) -> Option<ResizeTarget> {
        let content = Self::content_area(size);
        let boundaries = self
            .hit_test_cache
            .as_ref()
            .filter(|cache| cache.content == content)
            .map(|cache| &cache.resize_boundaries)?;
        let boundary = boundaries
            .iter()
            .filter(|boundary| resize_boundary_hit(boundary, x, y))
            .min_by_key(|boundary| boundary.depth)?;

        let pane_a = *boundary.first_pane_ids.first()?;
        let pane_b = *boundary.second_pane_ids.first()?;
        let mut pane_ids = boundary.first_pane_ids.clone();
        pane_ids.extend(boundary.second_pane_ids.iter().copied());

        Some(ResizeTarget {
            pane_a,
            pane_b,
            direction: boundary.direction,
            pane_ids,
        })
    }

    fn rebuild_hit_test_cache(&mut self) {
        let content = Self::content_area(self.last_terminal_size);
        let placements = if let Some(pane_id) = self.maximized_pane {
            vec![Placement {
                pane_id,
                area: content,
                exposed: ExposedSides {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
            }]
        } else if self.debug_container_boxes {
            self.debug_layout_areas(content)
                .1
                .into_iter()
                .map(|placement| Placement {
                    pane_id: placement.pane_id,
                    area: placement.pane_area,
                    exposed: ExposedSides {
                        top: true,
                        bottom: true,
                        left: true,
                        right: true,
                    },
                })
                .collect()
        } else {
            self.pane_placements(content)
        };

        let mut row_candidates = vec![Vec::new(); content.height as usize];
        for (idx, placement) in placements.iter().enumerate() {
            let start = placement.area.y.saturating_sub(content.y) as usize;
            let end = placement
                .area
                .bottom()
                .saturating_sub(content.y)
                .min(content.height) as usize;
            for row in start..end {
                if let Some(bucket) = row_candidates.get_mut(row) {
                    bucket.push(idx);
                }
            }
        }

        let mut resize_boundaries = Vec::new();
        if self.maximized_pane.is_none() {
            if self.debug_container_boxes {
                self.layout
                    .collect_debug_resize_boundaries(content, 0, &mut resize_boundaries);
            } else {
                self.layout
                    .collect_resize_boundaries(content, 0, &mut resize_boundaries);
            }
        }

        self.hit_test_cache = Some(HitTestCache {
            content,
            placements,
            row_candidates,
            resize_boundaries,
        });
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

    fn update_pane_swap_hover_target(
        &self,
        size: Rect,
        source_pane_id: usize,
        mouse: &MouseEvent,
    ) -> Option<usize> {
        self.placement_at(size, mouse.column, mouse.row)
            .map(|placement| placement.pane_id)
            .filter(|pane_id| *pane_id != source_pane_id)
    }

    fn finish_pane_swap_drag(&mut self, drag: DragPaneSwap, size: Rect) -> anyhow::Result<()> {
        self.drag_swap = None;

        if drag.moved {
            if let Some(target_pane_id) = drag.hovered_pane_id {
                if self
                    .layout
                    .swap_leaf_ids(drag.source_pane_id, target_pane_id)
                {
                    self.focus_pane(drag.source_pane_id);
                    self.persist_layout();
                    self.resize(size.height, size.width);
                }
            }
            return Ok(());
        }

        self.focus_pane(drag.source_pane_id);
        self.open_new_pane_picker(drag.source_pane_id);
        Ok(())
    }

    fn open_new_pane_picker(&mut self, pane_id: usize) {
        let Some((name, agent_index)) = self
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| (pane.title.clone(), agent_index_for_command(&pane.command)))
        else {
            return;
        };

        let agent_index = self.first_available_agent_for_pane(pane_id, agent_index);
        self.modal = Some(Modal::NewPanePicker {
            pane_id,
            source_pane_id: pane_id,
            close_on_cancel: false,
            cursor: name.chars().count(),
            name_selected: true,
            name,
            name_error: None,
            agent_index,
        });
    }
}

fn run_commander_harness(
    user_input: String,
    phase: CommanderPhase,
    conversation: Vec<String>,
    pending_plan: Option<CommanderPendingPlan>,
    pane_names: Vec<String>,
    pane_roster: Vec<String>,
    agent_types: Vec<String>,
    workspace_roster: Vec<String>,
) -> CommanderWorkerResult {
    let pane_list = pane_names
        .iter()
        .map(|name| format!("- {}", name))
        .collect::<Vec<_>>()
        .join("\n");
    let pane_status_list = pane_roster.join("\n");
    let agent_list = agent_types
        .iter()
        .map(|name| format!("- {}", name))
        .collect::<Vec<_>>()
        .join("\n");
    let workspace_list = workspace_roster.join("\n");
    let transcript = if conversation.is_empty() {
        "(no prior messages)".to_string()
    } else {
        conversation.join("\n")
    };
    let pending_plan_block = pending_plan
        .as_ref()
        .map(format_pending_plan_for_prompt)
        .unwrap_or_else(|| "NONE".to_string());
    let phase_hint = match phase {
        CommanderPhase::Discussing => {
            "You are in planning mode. Do not execute pane actions yet."
        }
        CommanderPhase::AwaitingApproval => {
            "A plan is waiting for user approval. Refine it if they ask for changes; do not execute."
        }
        CommanderPhase::Executing => "Execution is in progress.",
    };

    let prompt = format!(
        "You are Commander, an agent harness for a terminal multiplexer.\n\
         Your job is to have a collaborative planning conversation, then propose a concrete execution plan.\n\
         Do not create panes, switch workspaces, or send tasks until the user approves with /approve.\n\
         {phase_hint}\n\n\
         Available pane names:\n{pane_list}\n\
         Current pane roster:\n{pane_status_list}\n\
         Available pane types (canonical tokens):\n{agent_list}\n\
         Available workspaces:\n{workspace_list}\n\
         Speech aliases: codecs/code-ex/codacs => codex, open code/open-code => opencode.\n\n\
         Conversation so far:\n{transcript}\n\n\
         Pending plan (if any):\n{pending_plan_block}\n\n\
         Latest user message:\n{user_input}\n\n\
         Return EXACTLY one of these formats and nothing else.\n\n\
         Format A — still clarifying (questions, push the idea forward, no execution):\n\
         MODE=CONVERSATION\n\
         REPLY=<warm natural reply; ask focused questions until the deliverable is definitive>\n\n\
         Format B — plan is ready for approval:\n\
         MODE=PROPOSE_PLAN\n\
         REPLY=<present the plan clearly; mention /approve to run>\n\
         PLAN_SUMMARY=<one line>\n\
         PLAN_OBJECTIVE=<what will be built or done>\n\
         WORKSPACE=<CURRENT|CREATE|CREATE:<name>|USE:<workspace selector>>\n\
         PANES=<pane specs separated by semicolons; each spec is agent@pane_name@task slice>\n\n\
         Rules:\n\
         - Use MODE=CONVERSATION while requirements are still fuzzy.\n\
         - Use MODE=PROPOSE_PLAN only when you can name concrete pane assignments and task slices.\n\
         - Each pane task slice must be self-contained context for that agent.\n\
         - Prefer WORKSPACE=CREATE for multi-agent execution unless the user explicitly wants the current workspace.\n\
         - Split work across panes by distinct slices (research, implementation, tests, docs, etc.).\n\
         - Do not include ACTION/CREATE/TARGET fields in harness mode."
    );

    let output = Command::new("agent")
        .arg("--trust")
        .arg("-p")
        .arg("--output-format")
        .arg("text")
        .arg("--mode")
        .arg("ask")
        .arg(prompt)
        .output();

    let Ok(output) = output else {
        return commander_router_message(
            "Couldn't run the Commander router. Check that the `agent` CLI is installed."
                .to_string(),
        );
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_line = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("unknown error");
        let message = if stderr_line
            .to_ascii_lowercase()
            .contains("workspace trust required")
        {
            "Commander router is blocked by workspace trust. Run `agent --trust` once in this workspace.".to_string()
        } else {
            format!("Commander router failed: {stderr_line}")
        };
        return commander_router_message(message);
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if let Some(result) = parse_commander_harness_output(&text) {
        result
    } else if let Some(result) = parse_commander_router_output(&text) {
        CommanderWorkerResult {
            execution_allowed: false,
            ..result
        }
    } else if let Some(reply) = normalize_commander_reply_text(&text) {
        commander_harness_message(reply, false)
    } else {
        commander_harness_message(
            "Commander harness returned an empty response.".to_string(),
            false,
        )
    }
}

fn commander_harness_message(message: String, execution_allowed: bool) -> CommanderWorkerResult {
    CommanderWorkerResult {
        reply_text: Some(message.clone()),
        speech_text: Some(message),
        execution_allowed,
        ..CommanderWorkerResult::empty()
    }
}

fn commander_router_message(message: String) -> CommanderWorkerResult {
    commander_harness_message(message, false)
}

fn format_pending_plan_for_prompt(plan: &CommanderPendingPlan) -> String {
    let workspace = match &plan.workspace {
        CommanderWorkspacePlan::UseCurrent => "CURRENT".to_string(),
        CommanderWorkspacePlan::CreateNew => format!(
            "CREATE{}",
            plan.workspace_name
                .as_ref()
                .map(|name| format!(":{name}"))
                .unwrap_or_default()
        ),
        CommanderWorkspacePlan::SwitchTo(selector) => format!("USE:{selector}"),
    };
    let panes = plan
        .panes
        .iter()
        .map(|pane| {
            format!(
                "{}@{}@{}",
                pane.agent,
                pane.pane_name,
                pane.task.replace('@', " ")
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "summary={}\nobjective={}\nworkspace={workspace}\npanes={panes}",
        plan.summary, plan.objective
    )
}

fn is_plan_approval(input: &str) -> bool {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    matches!(
        normalized.as_str(),
        "/approve"
            | "/go"
            | "/execute"
            | "/run"
            | "approve"
            | "approved"
            | "yes"
            | "y"
            | "go"
            | "go ahead"
            | "run it"
            | "run the plan"
            | "execute"
            | "execute plan"
            | "ship it"
            | "looks good"
            | "lgtm"
    ) || normalized.starts_with("yes ")
        || normalized.starts_with("approve ")
}

fn is_plan_revision_request(input: &str) -> bool {
    let normalized = input.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "/revise" | "/replan" | "/cancel" | "revise" | "replan" | "not yet" | "wait" | "hold"
    ) || normalized.starts_with("change ")
        || normalized.starts_with("instead ")
        || normalized.contains("not ready")
}

fn parse_commander_harness_output(text: &str) -> Option<CommanderWorkerResult> {
    let mut mode = None::<String>;
    let mut reply = None::<String>;
    let mut plan_summary = None::<String>;
    let mut plan_objective = None::<String>;
    let mut workspace = None::<String>;
    let mut panes = None::<String>;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("MODE=") {
            mode = Some(value.trim().to_ascii_uppercase());
        } else if let Some(value) = trimmed.strip_prefix("REPLY=") {
            reply = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("PLAN_SUMMARY=") {
            plan_summary = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("PLAN_OBJECTIVE=") {
            plan_objective = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("WORKSPACE=") {
            workspace = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("PANES=") {
            panes = Some(value.trim().to_string());
        }
    }

    let mode = mode?;
    let reply_text = reply.as_deref().and_then(normalize_commander_reply_text);

    match mode.as_str() {
        "CONVERSATION" => Some(CommanderWorkerResult {
            reply_text: reply_text.clone(),
            speech_text: reply_text,
            execution_allowed: false,
            ..CommanderWorkerResult::empty()
        }),
        "PROPOSE_PLAN" => {
            let summary = plan_summary.filter(|value| !value.is_empty())?;
            let objective = plan_objective
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| summary.clone());
            let pane_specs = panes.unwrap_or_default();
            let assignments = parse_plan_pane_specs(&pane_specs);
            if assignments.is_empty() {
                return None;
            }
            let (workspace_plan, workspace_name) =
                parse_plan_workspace(workspace.as_deref().unwrap_or("CREATE"));
            Some(CommanderWorkerResult {
                reply_text: reply_text.clone(),
                speech_text: reply_text,
                proposed_plan: Some(CommanderPendingPlan {
                    summary,
                    objective,
                    workspace: workspace_plan,
                    workspace_name,
                    panes: assignments,
                }),
                execution_allowed: false,
                ..CommanderWorkerResult::empty()
            })
        }
        _ => None,
    }
}

fn parse_plan_workspace(value: &str) -> (CommanderWorkspacePlan, Option<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("CURRENT") {
        return (CommanderWorkspacePlan::UseCurrent, None);
    }
    if trimmed.eq_ignore_ascii_case("CREATE") || trimmed.eq_ignore_ascii_case("NEW") {
        return (CommanderWorkspacePlan::CreateNew, None);
    }
    if let Some(name) = trimmed
        .strip_prefix("CREATE:")
        .or_else(|| trimmed.strip_prefix("NEW:"))
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("NONE"))
    {
        return (CommanderWorkspacePlan::CreateNew, Some(name.to_string()));
    }
    if let Some(selector) = trimmed
        .strip_prefix("USE:")
        .or_else(|| trimmed.strip_prefix("SWITCH:"))
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
    {
        return (CommanderWorkspacePlan::SwitchTo(selector.to_string()), None);
    }
    (CommanderWorkspacePlan::SwitchTo(trimmed.to_string()), None)
}

fn parse_plan_pane_specs(value: &str) -> Vec<CommanderPaneAssignment> {
    value
        .split(';')
        .filter_map(|chunk| {
            let trimmed = chunk.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
                return None;
            }
            let mut parts = trimmed.splitn(3, '@');
            let agent = parts.next()?.trim();
            let pane_name = parts.next()?.trim();
            let task = parts.next()?.trim();
            if agent.is_empty() || pane_name.is_empty() || task.is_empty() {
                return None;
            }
            Some(CommanderPaneAssignment {
                agent: agent.to_string(),
                pane_name: pane_name.to_string(),
                task: task.to_string(),
            })
        })
        .collect()
}

fn parse_commander_router_output(text: &str) -> Option<CommanderWorkerResult> {
    let steps = parse_commander_step_blocks(text);
    if !steps.is_empty() {
        return Some(CommanderWorkerResult {
            steps,
            ..CommanderWorkerResult::empty()
        });
    }

    let mut action = None::<String>;
    let mut target = None::<String>;
    let mut message = None::<String>;
    let mut speech = None::<String>;
    let mut create = None::<String>;
    let mut rename = None::<String>;
    let mut close = None::<String>;
    let mut new_workspace = None::<String>;
    let mut workspace = None::<String>;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("ACTION=") {
            action = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("TARGET=") {
            target = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("MESSAGE=") {
            message = Some(value.to_string());
        } else if let Some(value) = trimmed.strip_prefix("SPEECH=") {
            speech = Some(value.to_string());
        } else if let Some(value) = trimmed.strip_prefix("CREATE=") {
            create = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("RENAME=") {
            rename = Some(value.to_string());
        } else if let Some(value) = trimmed.strip_prefix("CLOSE=") {
            close = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("NEW_WORKSPACE=") {
            new_workspace = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("WORKSPACE=") {
            workspace = Some(value.trim().to_string());
        }
    }

    if action.is_none() {
        return None;
    }
    let create_requests = create
        .as_deref()
        .map(parse_create_requests)
        .unwrap_or_default();
    let rename_requests = rename
        .as_deref()
        .map(parse_rename_requests)
        .unwrap_or_default();
    let close_requests = close
        .as_deref()
        .map(parse_close_requests)
        .unwrap_or_default();
    let workspace_create = (action
        .as_deref()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("CREATE_WORKSPACE")))
    .then(|| {
        new_workspace
            .as_deref()
            .and_then(parse_workspace_create_name)
    });
    let workspace_switch = workspace.as_deref().and_then(parse_workspace_switch);
    let action = action.unwrap_or_default().to_ascii_uppercase();
    let target = target.unwrap_or_default();
    let message = message.unwrap_or_default();
    let reply_text = if action == "REPLY" {
        normalize_commander_reply_text(&message)
    } else {
        None
    };
    let speech_text = speech.as_deref().and_then(normalize_commander_reply_text);
    let submit_payload = action.contains("TASK");
    let can_send = !target.is_empty()
        && !target.eq_ignore_ascii_case("NONE")
        && !message.is_empty()
        && action != "CREATE"
        && action != "CLOSE"
        && action != "SWITCH_WORKSPACE"
        && action != "CREATE_WORKSPACE"
        && action != "REPLY";

    Some(CommanderWorkerResult {
        steps: Vec::new(),
        reply_text,
        speech_text,
        target_name: can_send.then_some(target),
        payload: can_send.then_some(message),
        submit_payload: can_send && submit_payload,
        create_requests,
        rename_requests,
        close_requests,
        workspace_switch,
        workspace_create,
        proposed_plan: None,
        execution_allowed: true,
    })
}

fn parse_commander_step_blocks(text: &str) -> Vec<CommanderExecutionStep> {
    let mut steps = Vec::new();
    let mut fields = HashMap::<String, String>::new();
    let mut saw_step_marker = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("STEP=") {
            saw_step_marker = true;
            if !fields.is_empty() {
                if let Some(step) = build_commander_step_from_fields(&fields) {
                    steps.push(step);
                }
                fields.clear();
            }
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_uppercase();
        if matches!(
            key.as_str(),
            "ACTION"
                | "TARGET"
                | "MESSAGE"
                | "SPEECH"
                | "CREATE"
                | "RENAME"
                | "CLOSE"
                | "SAVE"
                | "NEW_WORKSPACE"
                | "WORKSPACE"
        ) {
            fields.insert(key, value.trim().to_string());
        }
    }

    if !fields.is_empty() {
        if let Some(step) = build_commander_step_from_fields(&fields) {
            steps.push(step);
        }
    }

    if saw_step_marker {
        steps
    } else {
        Vec::new()
    }
}

fn build_commander_step_from_fields(
    fields: &HashMap<String, String>,
) -> Option<CommanderExecutionStep> {
    let action = fields
        .get("ACTION")
        .map(|value| value.trim().to_ascii_uppercase())
        .unwrap_or_default();
    if action.is_empty() {
        return None;
    }

    let target = fields.get("TARGET").cloned().unwrap_or_default();
    let message = fields.get("MESSAGE").cloned().unwrap_or_default();
    let speech_text = fields
        .get("SPEECH")
        .and_then(|value| normalize_commander_reply_text(value));
    let reply_text = if action == "REPLY" {
        normalize_commander_reply_text(&message)
    } else {
        None
    };
    let submit_payload = action.contains("TASK");
    let can_send = !target.is_empty()
        && !target.eq_ignore_ascii_case("NONE")
        && !message.is_empty()
        && action != "CREATE"
        && action != "CLOSE"
        && action != "SWITCH_WORKSPACE"
        && action != "CREATE_WORKSPACE"
        && action != "REPLY";

    let create_requests = fields
        .get("CREATE")
        .map(|value| parse_create_requests(value))
        .unwrap_or_default();
    let rename_requests = fields
        .get("RENAME")
        .map(|value| parse_rename_requests(value))
        .unwrap_or_default();
    let close_requests = fields
        .get("CLOSE")
        .map(|value| parse_close_requests(value))
        .unwrap_or_default();
    let workspace_create = (action == "CREATE_WORKSPACE").then(|| {
        fields
            .get("NEW_WORKSPACE")
            .and_then(|value| parse_workspace_create_name(value))
    });
    let workspace_switch = fields
        .get("WORKSPACE")
        .and_then(|value| parse_workspace_switch(value));
    let save_as = fields.get("SAVE").and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Some(CommanderExecutionStep {
        save_as,
        reply_text,
        speech_text,
        target_name: can_send.then_some(target),
        payload: can_send.then_some(message),
        submit_payload: can_send && submit_payload,
        create_requests,
        rename_requests,
        close_requests,
        workspace_switch,
        workspace_create,
    })
}

fn format_commander_execution_reply(notes: &[String]) -> String {
    let recap = notes
        .iter()
        .map(|note| note.trim())
        .filter(|note| !note.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if recap.is_empty() {
        "Done.".to_string()
    } else {
        recap
    }
}

fn normalize_commander_reply_text(message: &str) -> Option<String> {
    let trimmed = message.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_agent_handoff(text: &str) -> Option<AgentHandoff> {
    let mut latest_block = None::<String>;
    let mut current_block = None::<Vec<String>>;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "COMMANDER_HANDOFF" {
            current_block = Some(Vec::new());
            continue;
        }
        if trimmed == "END_COMMANDER_HANDOFF" {
            if let Some(lines) = current_block.take() {
                latest_block = Some(lines.join("\n"));
            }
            continue;
        }
        if let Some(lines) = current_block.as_mut() {
            lines.push(line.to_string());
        }
    }

    let block = latest_block?;
    let mut status = None;
    let mut summary = None;
    let mut changed_files = None;
    let mut tests = None;
    let mut risks = None;
    let mut next = None;

    for line in block.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "status" => status = parse_handoff_status(value),
            "summary" => summary = Some(value.to_string()),
            "changed_files" => changed_files = Some(parse_handoff_changed_files(value)),
            "tests" => tests = Some(value.to_string()),
            "risks" => risks = Some(value.to_string()),
            "next" => next = Some(value.to_string()),
            _ => {}
        }
    }

    Some(AgentHandoff {
        status: status?,
        summary: summary.unwrap_or_default(),
        changed_files: changed_files.unwrap_or_default(),
        tests: tests.unwrap_or_default(),
        risks: risks.unwrap_or_default(),
        next: next.unwrap_or_default(),
    })
}

fn parse_handoff_status(value: &str) -> Option<HandoffStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "success" => Some(HandoffStatus::Success),
        "questionable" => Some(HandoffStatus::Questionable),
        "failed" => Some(HandoffStatus::Failed),
        "needs_tests" | "needs tests" | "needs-testing" => Some(HandoffStatus::NeedsTests),
        "blocked" => Some(HandoffStatus::Blocked),
        _ => None,
    }
}

fn parse_handoff_changed_files(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty() && !path.eq_ignore_ascii_case("none"))
        .map(ToString::to_string)
        .collect()
}

fn format_agent_handoff(handoff: &AgentHandoff) -> String {
    let files = if handoff.changed_files.is_empty() {
        "none".to_string()
    } else {
        handoff.changed_files.join(",")
    };
    format!(
        "status={}, summary={}, files={}, tests={}, risks={}, next={}",
        format_handoff_status(handoff.status),
        handoff.summary,
        files,
        handoff.tests,
        handoff.risks,
        handoff.next
    )
}

fn format_handoff_status(status: HandoffStatus) -> &'static str {
    match status {
        HandoffStatus::Success => "success",
        HandoffStatus::Questionable => "questionable",
        HandoffStatus::Failed => "failed",
        HandoffStatus::NeedsTests => "needs_tests",
        HandoffStatus::Blocked => "blocked",
    }
}

fn build_outgoing_payload(
    payload: &str,
    submit_payload: bool,
    pane_count: usize,
    shard_index: usize,
    pane_name: &str,
    pane_command: &str,
) -> String {
    if !submit_payload {
        return payload.to_string();
    }

    if pane_command == LOGIN_SHELL_SENTINEL {
        return payload.to_string();
    }

    if pane_count <= 1 {
        return append_commander_handoff_contract(payload);
    }

    append_commander_handoff_contract(&build_sharded_task_prompt(
        payload,
        shard_index,
        pane_count,
        pane_name,
        pane_command,
    ))
}

fn append_commander_handoff_contract(payload: &str) -> String {
    format!(
        "{}\n\nWhen finished, end with exactly this handoff block:\n\
COMMANDER_HANDOFF\n\
status: success|questionable|failed|needs_tests|blocked\n\
summary: <short result>\n\
changed_files: <paths or none>\n\
tests: <tests run and result, or not run>\n\
risks: <remaining concerns or none>\n\
next: <recommended next action or none>\n\
END_COMMANDER_HANDOFF",
        payload.trim_end()
    )
}

fn build_sharded_task_prompt(
    task_text: &str,
    shard_index: usize,
    shard_total: usize,
    pane_name: &str,
    pane_type: &str,
) -> String {
    let task = task_text.trim();
    let task = if task.is_empty() {
        "Complete the requested task."
    } else {
        task
    };

    let role = shard_role_for_index(shard_index, shard_total);

    format!(
        "Task: {task}\n\
Shard: {shard_index}/{shard_total}\n\
Role: {role}\n\
Pane: {pane_name}\n\
Agent: {pane_type}\n\
Work only on your distinct slice and avoid overlap with the other shards.\n\
Complete your slice end-to-end and keep the final reply concise."
    )
}

fn shard_role_for_index(shard_index: usize, shard_total: usize) -> &'static str {
    if shard_total <= 1 {
        return "complete the task";
    }

    if shard_index == 1 {
        "gather context and identify the right slice"
    } else if shard_index == shard_total {
        "verify the result and close gaps"
    } else {
        "implement a distinct slice of the task"
    }
}

fn parse_commander_slash_command(input: &str) -> Option<CommanderSlashCommand> {
    let trimmed = input.trim();
    let command = trimmed.strip_prefix('/')?.trim();
    let normalized = command.to_ascii_lowercase();
    let workspace_arg = command
        .strip_prefix("workspace ")
        .or_else(|| command.strip_prefix("workspace:"))
        .map(str::trim);
    if let Some(arg) = workspace_arg {
        if arg.eq_ignore_ascii_case("new") {
            return Some(CommanderSlashCommand::CreateWorkspace(None));
        }
        if let Some(name) = arg.strip_prefix("new ").map(str::trim) {
            return Some(CommanderSlashCommand::CreateWorkspace(
                parse_workspace_create_name(name),
            ));
        }
        if let Some(name) = arg.strip_prefix("create ").map(str::trim) {
            return Some(CommanderSlashCommand::CreateWorkspace(
                parse_workspace_create_name(name),
            ));
        }
    }

    match normalized.as_str() {
        "approve" | "go" | "execute" | "run" => Some(CommanderSlashCommand::ApprovePlan),
        "cancel" | "revise" | "replan" => Some(CommanderSlashCommand::CancelPlan),
        "theme" => Some(CommanderSlashCommand::OpenTheme),
        "settings" => Some(CommanderSlashCommand::OpenSettings),
        "new workspace" | "create workspace" | "workspace new" => {
            Some(CommanderSlashCommand::CreateWorkspace(None))
        }
        _ => normalized
            .strip_prefix("workspace ")
            .or_else(|| normalized.strip_prefix("workspace:"))
            .or_else(|| normalized.strip_prefix("switch "))
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .map(|selector| CommanderSlashCommand::SwitchWorkspace(selector.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commander_scroll_test_app() -> App {
        App {
            panes: Vec::new(),
            workspaces: vec![WorkspaceState {
                name: "Workspace 1".to_string(),
                layout: Node::Leaf { pane_id: 0 },
                focused: 0,
                maximized_pane: None,
            }],
            active_workspace: 0,
            layout: Node::Leaf { pane_id: 0 },
            focused: 0,
            maximized_pane: None,
            next_pane_id: 1,
            running: true,
            reload_requested: false,
            modal: None,
            drag_resize: None,
            drag_swap: None,
            drag_pane_mouse: None,
            text_selection: None,
            theme_index: 0,
            default_agent_index: default_agent_index(),
            theme_preview_index: 0,
            debug_container_boxes: false,
            mouse_capture_enabled: true,
            commander_focused: true,
            sidebar_workspace_focused: None,
            sidebar_add_button_focused: false,
            commander: CommanderState {
                input: String::new(),
                cursor: 0,
                busy: false,
                phase: CommanderPhase::Discussing,
                pending_plan: None,
                history: Vec::new(),
                chat_offset_from_bottom: 0,
                chat_pinned_to_bottom: true,
                chat_viewport_lines: 0,
                chat_total_lines: 0,
                rx: None,
            },
            agent_handoffs: std::collections::HashMap::new(),
            commander_palette_video: CommanderPaletteVideoState {
                child: None,
                rx: None,
                parser: vt100::Parser::new(1, 1, 0),
                frame_text: String::new(),
                rows: 0,
                cols: 0,
            },
            tts: TtsState {
                tx: None,
                event_rx: None,
            },
            last_terminal_size: Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 40,
            },
            hit_test_cache: None,
        }
    }
    use crate::ui::agent_command_for_input;

    #[test]
    fn outgoing_payload_is_forwarded_verbatim() {
        let outgoing =
            build_outgoing_payload("df -h", true, 1, 1, "terminal", LOGIN_SHELL_SENTINEL);

        assert_eq!(outgoing, "df -h");
    }

    #[test]
    fn agent_task_payload_gets_handoff_contract() {
        let outgoing = build_outgoing_payload("Refactor the parser.", true, 1, 1, "alpha", "codex");

        assert!(outgoing.contains("Refactor the parser."));
        assert!(outgoing.contains("COMMANDER_HANDOFF"));
        assert!(outgoing.contains("END_COMMANDER_HANDOFF"));
    }

    #[test]
    fn commander_step_accepts_speech_field() {
        let mut fields = HashMap::new();
        fields.insert("ACTION".to_string(), "SEND".to_string());
        fields.insert("TARGET".to_string(), "terminal".to_string());
        fields.insert("MESSAGE".to_string(), "clear".to_string());
        fields.insert("SPEECH".to_string(), "Terminal cleared.".to_string());

        let step = build_commander_step_from_fields(&fields).expect("expected step");
        assert_eq!(step.speech_text.as_deref(), Some("Terminal cleared."));
    }

    #[test]
    fn agent_input_aliases_map_to_canonical_command() {
        assert_eq!(agent_command_for_input("codex"), Some("codex"));
        assert_eq!(agent_command_for_input("open code"), Some("opencode"));
        assert_eq!(agent_command_for_input("pi"), Some("pi"));
        assert_eq!(agent_command_for_input("shell"), Some(LOGIN_SHELL_SENTINEL));
        assert_eq!(agent_command_for_input("unknown"), None);
    }

    #[test]
    fn sharded_agent_payload_gets_distinct_role_prompt() {
        let outgoing = build_outgoing_payload("Refactor the parser.", true, 3, 2, "beta", "codex");

        assert!(outgoing.contains("Shard: 2/3"));
        assert!(outgoing.contains("Role: implement a distinct slice of the task"));
        assert!(outgoing.contains("Work only on your distinct slice"));
        assert!(outgoing.contains("COMMANDER_HANDOFF"));
    }

    #[test]
    fn parses_latest_agent_handoff_block() {
        let handoff = parse_agent_handoff(
            "COMMANDER_HANDOFF\n\
             status: failed\n\
             summary: old\n\
             changed_files: none\n\
             tests: not run\n\
             risks: stale\n\
             next: retry\n\
             END_COMMANDER_HANDOFF\n\
             COMMANDER_HANDOFF\n\
             status: needs_tests\n\
             summary: Implemented parser changes.\n\
             changed_files: src/app.rs, src/pane.rs\n\
             tests: cargo test commander_ --bin split_tui passed\n\
             risks: Needs full test suite.\n\
             next: Run cargo test.\n\
             END_COMMANDER_HANDOFF",
        )
        .expect("expected handoff");

        assert_eq!(handoff.status, HandoffStatus::NeedsTests);
        assert_eq!(handoff.summary, "Implemented parser changes.");
        assert_eq!(handoff.changed_files, vec!["src/app.rs", "src/pane.rs"]);
        assert_eq!(handoff.next, "Run cargo test.");
    }

    #[test]
    fn commander_chat_scroll_pins_to_bottom_and_clamps() {
        let mut app = commander_scroll_test_app();
        app.set_commander_chat_metrics(10, 100);
        app.commander_scroll_to_top();
        assert_eq!(app.commander_chat_offset_from_bottom(), 90);
        assert!(!app.commander_chat_pinned_to_bottom());

        app.commander_scroll_to_bottom();
        assert_eq!(app.commander_chat_offset_from_bottom(), 0);
        assert!(app.commander_chat_pinned_to_bottom());

        app.commander_page_up();
        assert_eq!(app.commander_chat_offset_from_bottom(), 10);
        assert!(!app.commander_chat_pinned_to_bottom());

        app.commander_page_down();
        assert_eq!(app.commander_chat_offset_from_bottom(), 0);
        assert!(app.commander_chat_pinned_to_bottom());

        app.commander_scroll_up();
        app.commander_scroll_up();
        assert_eq!(app.commander_chat_offset_from_bottom(), 2);
        app.commander_scroll_down();
        assert_eq!(app.commander_chat_offset_from_bottom(), 1);
        app.commander_scroll_down();
        assert_eq!(app.commander_chat_offset_from_bottom(), 0);
        assert!(app.commander_chat_pinned_to_bottom());
    }

    #[test]
    fn commander_chat_scroll_follows_new_history_when_pinned() {
        let mut app = commander_scroll_test_app();
        app.set_commander_chat_metrics(5, 20);
        app.commander_scroll_up();
        assert_eq!(app.commander_chat_offset_from_bottom(), 1);
        assert!(!app.commander_chat_pinned_to_bottom());

        app.commander.history.push("You: hello".to_string());
        app.commander_sync_scroll_after_history_change();
        assert_eq!(app.commander_chat_offset_from_bottom(), 1);

        app.commander_scroll_to_bottom();
        app.commander.history.push("Commander: hi".to_string());
        app.commander_sync_scroll_after_history_change();
        assert_eq!(app.commander_chat_offset_from_bottom(), 0);
        assert!(app.commander_chat_pinned_to_bottom());
    }

    #[test]
    fn commander_slash_commands_parse() {
        assert!(matches!(
            parse_commander_slash_command("/approve"),
            Some(CommanderSlashCommand::ApprovePlan)
        ));
        assert!(matches!(
            parse_commander_slash_command("/cancel"),
            Some(CommanderSlashCommand::CancelPlan)
        ));
        assert!(matches!(
            parse_commander_slash_command("/theme"),
            Some(CommanderSlashCommand::OpenTheme)
        ));
        assert!(matches!(
            parse_commander_slash_command("/settings"),
            Some(CommanderSlashCommand::OpenSettings)
        ));
        assert!(matches!(
            parse_commander_slash_command("/workspace 2"),
            Some(CommanderSlashCommand::SwitchWorkspace(selector)) if selector == "2"
        ));
        assert!(matches!(
            parse_commander_slash_command("/switch next"),
            Some(CommanderSlashCommand::SwitchWorkspace(selector)) if selector == "next"
        ));
        assert!(matches!(
            parse_commander_slash_command("/workspace new"),
            Some(CommanderSlashCommand::CreateWorkspace(None))
        ));
        assert!(matches!(
            parse_commander_slash_command("/workspace new Research"),
            Some(CommanderSlashCommand::CreateWorkspace(Some(name))) if name == "Research"
        ));
        assert!(parse_commander_slash_command("theme").is_none());
    }

    #[test]
    fn commander_single_step_parses_workspace_switch() {
        let result = parse_commander_router_output(
            "ACTION=SWITCH_WORKSPACE\n\
             TARGET=NONE\n\
             MESSAGE=NONE\n\
             SPEECH=Switched workspaces.\n\
             CREATE=NONE\n\
             RENAME=NONE\n\
             CLOSE=NONE\n\
             NEW_WORKSPACE=NONE\n\
             WORKSPACE=Research",
        )
        .expect("expected router result");

        assert_eq!(result.workspace_switch.as_deref(), Some("Research"));
        assert!(result.target_name.is_none());
    }

    #[test]
    fn commander_step_parses_workspace_switch() {
        let result = parse_commander_router_output(
            "STEP=switch\n\
             ACTION=SWITCH_WORKSPACE\n\
             TARGET=NONE\n\
             MESSAGE=NONE\n\
             SPEECH=Switched workspaces.\n\
             CREATE=NONE\n\
             RENAME=NONE\n\
             CLOSE=NONE\n\
             NEW_WORKSPACE=NONE\n\
             WORKSPACE=next\n\
             SAVE=NONE",
        )
        .expect("expected router result");

        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].workspace_switch.as_deref(), Some("next"));
        assert!(result.steps[0].target_name.is_none());
    }

    #[test]
    fn commander_single_step_parses_workspace_create() {
        let result = parse_commander_router_output(
            "ACTION=CREATE_WORKSPACE\n\
             TARGET=NONE\n\
             MESSAGE=NONE\n\
             SPEECH=Created a workspace.\n\
             CREATE=NONE\n\
             RENAME=NONE\n\
             CLOSE=NONE\n\
             NEW_WORKSPACE=Research\n\
             WORKSPACE=NONE",
        )
        .expect("expected router result");

        assert_eq!(
            result
                .workspace_create
                .as_ref()
                .and_then(|name| name.as_deref()),
            Some("Research")
        );
        assert!(result.target_name.is_none());
    }

    #[test]
    fn commander_harness_parses_conversation_mode() {
        let result = parse_commander_harness_output(
            "MODE=CONVERSATION\nREPLY=What should the first milestone be?",
        )
        .expect("expected harness result");
        assert!(!result.execution_allowed);
        assert_eq!(
            result.reply_text.as_deref(),
            Some("What should the first milestone be?")
        );
        assert!(result.proposed_plan.is_none());
    }

    #[test]
    fn commander_harness_parses_proposed_plan() {
        let result = parse_commander_harness_output(
            "MODE=PROPOSE_PLAN\n\
             REPLY=Here is the plan.\n\
             PLAN_SUMMARY=Build auth flow\n\
             PLAN_OBJECTIVE=Implement login and session handling\n\
             WORKSPACE=CREATE:Auth Sprint\n\
             PANES=codex@Plan@Design API contract;opencode@Build@Implement handlers",
        )
        .expect("expected harness result");
        let plan = result.proposed_plan.expect("expected plan");
        assert_eq!(plan.summary, "Build auth flow");
        assert_eq!(plan.panes.len(), 2);
        assert!(matches!(plan.workspace, CommanderWorkspacePlan::CreateNew));
        assert_eq!(plan.workspace_name.as_deref(), Some("Auth Sprint"));
        assert!(!result.execution_allowed);
    }

    #[test]
    fn plan_approval_phrases_are_detected() {
        assert!(is_plan_approval("/approve"));
        assert!(is_plan_approval("looks good"));
        assert!(!is_plan_approval("maybe later"));
    }

    #[test]
    fn commander_step_parses_workspace_create() {
        let result = parse_commander_router_output(
            "STEP=create-workspace\n\
             ACTION=CREATE_WORKSPACE\n\
             TARGET=NONE\n\
             MESSAGE=NONE\n\
             SPEECH=Created a workspace.\n\
             CREATE=NONE\n\
             RENAME=NONE\n\
             CLOSE=NONE\n\
             NEW_WORKSPACE=NONE\n\
             WORKSPACE=NONE\n\
             SAVE=NONE",
        )
        .expect("expected router result");

        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].workspace_create, Some(None));
        assert!(result.steps[0].target_name.is_none());
    }

    #[test]
    fn sidebar_index_shift_clamps_at_bounds() {
        assert_eq!(shift_sidebar_item_index(0, 4, -1), 0);
        assert_eq!(shift_sidebar_item_index(3, 4, 1), 3);
        assert_eq!(shift_sidebar_item_index(1, 4, -9), 0);
        assert_eq!(shift_sidebar_item_index(2, 4, 9), 3);
    }

    #[test]
    fn sidebar_index_shift_moves_one_step_when_in_range() {
        assert_eq!(shift_sidebar_item_index(0, 4, 1), 1);
        assert_eq!(shift_sidebar_item_index(2, 4, -1), 1);
    }
}

fn init_tts_state() -> TtsState {
    if !parse_bool_env_with_default("CODEUI_TTS", true) {
        return TtsState {
            tx: None,
            event_rx: None,
        };
    }
    let (tx, rx) = mpsc::channel::<String>();
    let (event_tx, event_rx) = mpsc::channel::<TtsEvent>();
    let config = tts_config_from_env();
    thread::spawn(move || run_tts_worker(rx, event_tx, config));
    TtsState {
        tx: Some(tx),
        event_rx: Some(event_rx),
    }
}

fn tts_config_from_env() -> TtsConfig {
    let timeout_secs = std::env::var("CODEUI_TTS_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(12)
        .clamp(3, 60);
    TtsConfig {
        edge_voice: std::env::var("CODEUI_TTS_VOICE")
            .unwrap_or_else(|_| "en-US-JennyNeural".to_string()),
        edge_rate: std::env::var("CODEUI_TTS_RATE").unwrap_or_else(|_| "+0%".to_string()),
        edge_volume: std::env::var("CODEUI_TTS_VOLUME").unwrap_or_else(|_| "+0%".to_string()),
        edge_pitch: std::env::var("CODEUI_TTS_PITCH").unwrap_or_else(|_| "+0Hz".to_string()),
        timeout_secs,
    }
}

fn run_tts_worker(rx: Receiver<String>, event_tx: Sender<TtsEvent>, config: TtsConfig) {
    for text in rx {
        if text.trim().is_empty() {
            continue;
        }
        let _ = event_tx.send(TtsEvent::SpeakStarted);
        if speak_with_edge_tts(&text, &config) {
            let _ = event_tx.send(TtsEvent::SpeakFinished);
            continue;
        }
        let _ = Command::new("espeak").arg(&text).status();
        let _ = event_tx.send(TtsEvent::SpeakFinished);
    }
}

fn resolve_ascii_video_binary() -> Option<&'static Path> {
    let release = Path::new("/home/aaron/lab/ascii-video/target/release/ascii-video");
    if release.exists() {
        return Some(release);
    }
    let debug = Path::new("/home/aaron/lab/ascii-video/target/debug/ascii-video");
    if debug.exists() {
        return Some(debug);
    }
    None
}

fn commander_palette_video_source_rows(rows: u16) -> u16 {
    rows.saturating_add(1)
}

fn normalize_commander_video_frame(
    rows: Vec<String>,
    target_rows: u16,
    target_cols: u16,
) -> String {
    let target_rows = target_rows.max(1) as usize;
    let target_cols = target_cols.max(1) as usize;
    let mut normalized = Vec::with_capacity(target_rows);

    for row in rows.into_iter().take(target_rows) {
        normalized.push(fit_commander_video_row(row, target_cols));
    }

    while normalized.len() < target_rows {
        normalized.push(" ".repeat(target_cols));
    }

    normalized.join("\n")
}

fn fit_commander_video_row(row: String, target_cols: usize) -> String {
    let mut out: String = row.chars().take(target_cols).collect();
    let width = out.chars().count();
    if width < target_cols {
        out.extend(std::iter::repeat_n(' ', target_cols - width));
    }
    out
}

fn sh_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn pump_commander_video_output(mut reader: impl Read + Send + 'static, tx: Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn speak_with_edge_tts(text: &str, config: &TtsConfig) -> bool {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let media_path = format!("/tmp/codeui-tts-{}-{}.mp3", std::process::id(), stamp);

    let generated = run_command_with_timeout(
        "edge-tts",
        &[
            "--voice",
            config.edge_voice.as_str(),
            "--rate",
            config.edge_rate.as_str(),
            "--volume",
            config.edge_volume.as_str(),
            "--pitch",
            config.edge_pitch.as_str(),
            "--text",
            text,
            "--write-media",
            media_path.as_str(),
        ],
        config.timeout_secs,
    );
    if !generated {
        let _ = std::fs::remove_file(&media_path);
        return false;
    }

    let played = run_command_with_timeout(
        "ffplay",
        &[
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "quiet",
            media_path.as_str(),
        ],
        config.timeout_secs,
    );
    let _ = std::fs::remove_file(&media_path);
    played
}

fn run_command_with_timeout(program: &str, args: &[&str], timeout_secs: u64) -> bool {
    let duration = format!("{}s", timeout_secs);
    let timeout_status = Command::new("timeout")
        .arg(duration)
        .arg(program)
        .args(args)
        .status();
    match timeout_status {
        Ok(status) => status.success(),
        Err(_) => Command::new(program)
            .args(args)
            .status()
            .map(|status| status.success())
            .unwrap_or(false),
    }
}

fn count_phrase(count: usize, singular_noun: &str, plural_noun: &str) -> String {
    if count == 1 {
        format!("one {}", singular_noun)
    } else {
        format!("{} {}", count, plural_noun)
    }
}

fn summarize_created_panes(created: &[String]) -> String {
    if created.is_empty() {
        return "no panes".to_string();
    }

    let mut order = Vec::<String>::new();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for command in created {
        let key = command.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if !counts.contains_key(&key) {
            order.push(key.clone());
        }
        *counts.entry(key).or_insert(0) += 1;
    }

    let mut parts = Vec::<String>::new();
    for pane_type in order {
        let count = counts.get(&pane_type).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let singular = format!("{} pane", pane_type);
        let plural = format!("{} panes", pane_type);
        parts.push(count_phrase(count, &singular, &plural));
    }

    join_sentence_list(&parts)
}

fn join_sentence_list(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let head = &parts[..parts.len().saturating_sub(1)];
            let last = &parts[parts.len() - 1];
            format!("{}, and {}", head.join(", "), last)
        }
    }
}

fn parse_create_requests(text: &str) -> Vec<(String, usize)> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    let mut out = Vec::new();
    for part in trimmed.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let (name, count_str) = token
            .split_once(':')
            .or_else(|| token.split_once('='))
            .unwrap_or((token, "1"));
        let count = count_str.trim().parse::<usize>().unwrap_or(1).max(1);
        let normalized_name = name.trim().to_ascii_lowercase();
        if !normalized_name.is_empty() {
            out.push((normalized_name, count));
        }
    }
    out
}

fn parse_close_requests(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    trimmed
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_workspace_switch(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_workspace_create_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("new")
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_rename_requests(text: &str) -> Vec<(String, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    trimmed
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter_map(|token| {
            token
                .split_once("=>")
                .or_else(|| token.split_once("->"))
                .or_else(|| token.split_once('='))
                .map(|(selector, name)| (selector.trim().to_string(), name.trim().to_string()))
        })
        .filter(|(selector, name)| !selector.is_empty() && !name.is_empty())
        .collect()
}

fn resolve_reference_selector(selector: &str, refs: &CommanderStepRefs) -> Option<Vec<usize>> {
    let normalized = selector.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if matches!(
        normalized.as_str(),
        "ref:last" | "last_created" | "newest_created"
    ) {
        return Some(refs.last_created_ids.clone());
    }
    if let Some(name) = normalized.strip_prefix("ref:") {
        let key = name.trim();
        if key.is_empty() {
            return Some(Vec::new());
        }
        return Some(refs.named.get(key).cloned().unwrap_or_default());
    }
    None
}

fn pane_matches_status(pane: &Pane, status: &str, focused_pane_id: usize) -> bool {
    let status = status.trim();
    match status {
        "focused" | "active" | "selected" => pane.id == focused_pane_id,
        "running" | "alive" | "open" => !pane.exited,
        "exited" | "dead" | "closed" | "finished" => pane.exited,
        "relaunch_failed" | "failed" | "error" | "broken" | "stuck" => pane.relaunch_failed,
        "commander" => pane.command == COMMANDER_COMMAND,
        "worker" | "agent" | "non_commander" => pane.command != COMMANDER_COMMAND,
        other if other.starts_with("type:") => {
            let command = other.trim_start_matches("type:").trim();
            pane.command.eq_ignore_ascii_case(command)
        }
        _ => false,
    }
}

fn shift_sidebar_item_index(current: usize, total_items: usize, step: isize) -> usize {
    if total_items == 0 {
        return current;
    }
    let max_index = total_items.saturating_sub(1);
    if step < 0 {
        let amount = step.saturating_abs() as usize;
        return current.saturating_sub(amount).min(max_index);
    }
    current.saturating_add(step as usize).min(max_index)
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

fn parse_bool_env_with_default(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default)
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
        .unwrap_or(default_agent_index())
}

fn agent_index_for_alias(token: &str) -> Option<usize> {
    let normalized = token
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
        "terminal" | "shell" => "__SHELL__",
        other => other,
    };
    AGENT_PRESETS
        .iter()
        .position(|preset| preset.command.eq_ignore_ascii_case(canonical))
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn insert_char_at_cursor(text: &mut String, cursor: &mut usize, ch: char) {
    let byte_idx = char_to_byte_idx(text, *cursor);
    text.insert(byte_idx, ch);
    *cursor += 1;
}

fn remove_char_before_cursor(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let start = char_to_byte_idx(text, *cursor - 1);
    let end = char_to_byte_idx(text, *cursor);
    text.replace_range(start..end, "");
    *cursor -= 1;
}

fn remove_char_at_cursor(text: &mut String, cursor: usize) {
    let len = text.chars().count();
    if cursor >= len {
        return;
    }
    let start = char_to_byte_idx(text, cursor);
    let end = char_to_byte_idx(text, cursor + 1);
    text.replace_range(start..end, "");
}

fn move_cell((col, row): (u16, u16), delta: i16, cols: u16, rows: u16) -> (u16, u16) {
    let max_index = u32::from(cols)
        .saturating_mul(u32::from(rows))
        .saturating_sub(1);
    let index = u32::from(row)
        .saturating_mul(u32::from(cols))
        .saturating_add(u32::from(col));
    let next = if delta.is_negative() {
        index.saturating_sub(u32::from(delta.unsigned_abs()))
    } else {
        index.saturating_add(delta as u32).min(max_index)
    };
    (
        (next % u32::from(cols)) as u16,
        (next / u32::from(cols)) as u16,
    )
}

fn write_osc52_clipboard(text: &str) -> anyhow::Result<()> {
    let encoded = base64_encode(text.as_bytes());
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{}\x07", encoded)?;
    stdout.flush()?;
    Ok(())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}
