use std::path::{Path, PathBuf};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    git_status::{ChangedFile, CheckResults, FileChangeKind, GitSummary},
    git_worktree::{WorktreeInfo, WorktreeGitSnapshot},
    theme::Theme,
    ui::{bg_color, render_lines_in_area, truncate_to_width},
    worktree_lifecycle::{
        compute_worktree_state, worktree_display_name, WorktreeAction, WorktreeLifecycleState,
        WorktreeRuntimeFlag,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeListColumn {
    Name,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreePickerFocus {
    List,
    DeleteButton,
    DeleteConfirm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeSubmodal {
    None,
    NewWorktree {
        name: String,
        base_branch: String,
        branch: String,
        field: NewWorktreeField,
        error: Option<String>,
        cursor: usize,
    },
    RunAgent {
        task: String,
        cursor: usize,
    },
    CommitReview {
        files: Vec<ChangedFile>,
        scroll: usize,
        file_cursor: usize,
        select_all_tracked: bool,
        select_all_untracked: bool,
    },
    CommitMessage {
        files: Vec<ChangedFile>,
        message: String,
        cursor: usize,
    },
    MergeConfirm {
        entry_index: usize,
    },
    ChecksFailed {
        entry_index: usize,
        results: CheckResults,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewWorktreeField {
    Name,
    Base,
    Branch,
}

impl NewWorktreeField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Base,
            Self::Base => Self::Branch,
            Self::Branch => Self::Name,
        }
    }
}

pub(crate) fn worktree_picker_item_count(entries: &[WorktreeInfo]) -> usize {
    1 + entries.len()
}

pub(crate) fn worktree_row_state(
    idx: usize,
    entries: &[WorktreeInfo],
    entry_summaries: &[Option<GitSummary>],
    entry_snapshots: &[WorktreeGitSnapshot],
    _repo_root: &Path,
    current_path: Option<&Path>,
    runtime_flags: &[(PathBuf, WorktreeRuntimeFlag)],
) -> WorktreeLifecycleState {
    if idx == 0 {
        return WorktreeLifecycleState::Current;
    }
    let entry_index = idx - 1;
    let entry = entries.get(entry_index);
    let summary = entry_summaries.get(entry_index).and_then(|s| s.as_ref());
    let snapshot = entry_snapshots
        .get(entry_index)
        .cloned()
        .unwrap_or_default();
    let is_current = entry.is_some_and(|e| {
        current_path.is_some_and(|p| crate::git_worktree::paths_equal(p, &e.path))
    });
    let runtime = entry.and_then(|e| {
        runtime_flags
            .iter()
            .find(|(path, _)| crate::git_worktree::paths_equal(path, &e.path))
            .map(|(_, flag)| *flag)
    });
    compute_worktree_state(is_current, summary, &snapshot, runtime)
}

pub(crate) fn worktree_picker_modal_width(
    entries: &[WorktreeInfo],
    entry_states: &[WorktreeLifecycleState],
    repo_root: &Path,
) -> u16 {
    let mut width = 42usize;
    for (idx, entry) in entries.iter().enumerate() {
        let name = worktree_display_name(entry, repo_root);
        let state = entry_states
            .get(idx + 1)
            .copied()
            .unwrap_or(WorktreeLifecycleState::Clean);
        let row = format!("{name:<20} {}", state.label());
        width = width.max(row.chars().count() + 4);
    }
    width.min(64) as u16
}

pub(crate) fn worktree_picker_modal_height(
    entries: &[WorktreeInfo],
    focus: WorktreePickerFocus,
    submodal: &WorktreeSubmodal,
) -> u16 {
    match submodal {
        WorktreeSubmodal::None => match focus {
            WorktreePickerFocus::List | WorktreePickerFocus::DeleteButton => {
                (worktree_picker_item_count(entries) as u16 + 3).min(18)
            }
            WorktreePickerFocus::DeleteConfirm => 9,
        },
        WorktreeSubmodal::NewWorktree { .. } => 14,
        WorktreeSubmodal::RunAgent { .. } => 10,
        WorktreeSubmodal::CommitReview { files, .. } => {
            (files.len() as u16 + 10).clamp(12, 24)
        }
        WorktreeSubmodal::CommitMessage { .. } => 12,
        WorktreeSubmodal::MergeConfirm { .. } => 14,
        WorktreeSubmodal::ChecksFailed { .. } => 12,
    }
}

fn worktree_picker_inner(modal_area: Rect, title: &str) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .inner(modal_area)
}

pub(crate) fn worktree_picker_list_area(modal_area: Rect) -> Rect {
    worktree_picker_inner(modal_area, "─ Worktrees ─")
}

pub(crate) fn worktree_picker_list_hit_index(
    modal_area: Rect,
    entries: &[WorktreeInfo],
    x: u16,
    y: u16,
) -> Option<usize> {
    let list_area = worktree_picker_list_area(modal_area);
    if !crate::utils::contains(list_area, x, y) {
        return None;
    }
    Some(
        (y.saturating_sub(list_area.y) as usize).min(worktree_picker_item_count(entries) - 1),
    )
}

pub(crate) fn worktree_picker_status_column_hit(
    modal_area: Rect,
    entries: &[WorktreeInfo],
    x: u16,
    y: u16,
) -> Option<usize> {
    let list_area = worktree_picker_list_area(modal_area);
    if !crate::utils::contains(list_area, x, y) {
        return None;
    }
    let row = (y.saturating_sub(list_area.y) as usize).min(worktree_picker_item_count(entries) - 1);
    let status_x = list_area.x.saturating_add(list_area.width / 2);
    if x >= status_x {
        Some(row)
    } else {
        None
    }
}

pub(crate) fn render_worktree_picker(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    entries: &[WorktreeInfo],
    entry_states: &[WorktreeLifecycleState],
    repo_root: &Path,
    current_path: Option<&Path>,
    selected_index: usize,
    list_column: WorktreeListColumn,
    focus: WorktreePickerFocus,
    delete_target_index: Option<usize>,
    delete_action_index: usize,
    submodal: &WorktreeSubmodal,
    target_branch: &str,
    entry_snapshots: &[WorktreeGitSnapshot],
    entry_summaries: &[Option<GitSummary>],
) {
    f.render_widget(Clear, area);

    let (title, border_color) = match submodal {
        WorktreeSubmodal::None => match focus {
            WorktreePickerFocus::DeleteConfirm => ("─ Delete Worktree ─", Color::Yellow),
            _ => ("─ Worktrees ─", theme.accent),
        },
        WorktreeSubmodal::NewWorktree { .. } => ("─ New Worktree ─", theme.accent),
        WorktreeSubmodal::RunAgent { .. } => ("─ Run Agent ─", theme.accent),
        WorktreeSubmodal::CommitReview { .. } => ("─ Commit ─", theme.accent),
        WorktreeSubmodal::CommitMessage { .. } => ("─ Commit Message ─", theme.accent),
        WorktreeSubmodal::MergeConfirm { .. } => ("─ Merge Worktree ─", theme.accent),
        WorktreeSubmodal::ChecksFailed { .. } => ("─ Checks Failed ─", Color::Red),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title)
        .style(Style::default().fg(theme.foreground).bg(bg_color(theme)))
        .border_style(Style::default().fg(border_color));
    f.render_widget(block, area);

    match submodal {
        WorktreeSubmodal::None if focus == WorktreePickerFocus::DeleteConfirm => {
            render_delete_confirm(
                f,
                area,
                theme,
                entries,
                delete_target_index,
                delete_action_index,
                entry_summaries,
            );
        }
        WorktreeSubmodal::None => {
            render_worktree_list(
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
            );
        }
        WorktreeSubmodal::NewWorktree {
            name,
            base_branch,
            branch,
            field,
            error,
            ..
        } => render_new_worktree_form(
            f,
            area,
            theme,
            name,
            base_branch,
            branch,
            *field,
            error.as_deref(),
        ),
        WorktreeSubmodal::RunAgent { task, .. } => {
            render_run_agent_form(f, area, theme, task, entries, selected_index, repo_root);
        }
        WorktreeSubmodal::CommitReview {
            files,
            scroll,
            file_cursor,
            select_all_tracked,
            select_all_untracked,
        } => render_commit_review(
            f,
            area,
            theme,
            files,
            *scroll,
            *file_cursor,
            *select_all_tracked,
            *select_all_untracked,
        ),
        WorktreeSubmodal::CommitMessage { files, message, .. } => {
            let count = files.iter().filter(|f| f.selected).count();
            render_commit_message(f, area, theme, message, count);
        }
        WorktreeSubmodal::MergeConfirm { entry_index } => render_merge_confirm(
            f,
            area,
            theme,
            entries,
            entry_summaries,
            entry_snapshots,
            *entry_index,
            repo_root,
            target_branch,
        ),
        WorktreeSubmodal::ChecksFailed {
            entry_index,
            results,
        } => render_checks_failed(
            f,
            area,
            theme,
            entries,
            *entry_index,
            repo_root,
            results,
        ),
    }
}

fn render_worktree_list(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    entries: &[WorktreeInfo],
    entry_states: &[WorktreeLifecycleState],
    repo_root: &Path,
    _current_path: Option<&Path>,
    selected_index: usize,
    list_column: WorktreeListColumn,
    focus: WorktreePickerFocus,
) {
    let frame = f.size();
    let list_area = clip_rect_to_frame(worktree_picker_list_area(area), frame);
    let item_count = worktree_picker_item_count(entries);
    let selected = selected_index.min(item_count.saturating_sub(1));
    let status_col = list_area.x.saturating_add(list_area.width / 2);
    let mut lines = Vec::new();

    for idx in 0..item_count {
        let row_selected = idx == selected;
        let marker = if row_selected && list_column == WorktreeListColumn::Name {
            "> "
        } else {
            "  "
        };
        let left = if idx == 0 {
            "New worktree".to_string()
        } else {
            let entry = &entries[idx - 1];
            worktree_display_name(entry, repo_root)
        };

        let state = entry_states
            .get(idx)
            .copied()
            .unwrap_or(WorktreeLifecycleState::Clean);
        let status_focused = row_selected && list_column == WorktreeListColumn::Status;
        let right = if status_focused {
            if let Some(action) = state.action_label() {
                format!("> {action}")
            } else {
                format!("> {}", state.label())
            }
        } else {
            state.label().to_string()
        };

        let left_max = (status_col.saturating_sub(list_area.x) as usize).saturating_sub(2);
        let left_text = truncate_to_width(&left, left_max.max(1));
        let pad = status_col.saturating_sub(
            list_area.x + (marker.len() + left_text.chars().count()) as u16,
        );

        let name_style = if row_selected && list_column == WorktreeListColumn::Name {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        let status_style = if status_focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if state == WorktreeLifecycleState::MergeReady {
            Style::default().fg(Color::Green)
        } else if matches!(
            state,
            WorktreeLifecycleState::Dirty | WorktreeLifecycleState::ChecksFailed
        ) {
            Style::default().fg(Color::Yellow)
        } else if state == WorktreeLifecycleState::Merged {
            Style::default().fg(theme.muted)
        } else {
            Style::default().fg(theme.muted)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{left_text}"), name_style),
            Span::raw(" ".repeat(pad as usize)),
            Span::styled(right, status_style),
        ]));
    }

    render_lines_in_area(
        f,
        list_area,
        lines,
        Style::default().fg(theme.foreground).bg(bg_color(theme)),
        frame,
    );

    if focus == WorktreePickerFocus::DeleteButton {
        let _ = focus;
    }
}

fn render_delete_confirm(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    entries: &[WorktreeInfo],
    delete_target_index: Option<usize>,
    delete_action_index: usize,
    entry_summaries: &[Option<GitSummary>],
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Delete Worktree ─");
    let entry = delete_target_index.and_then(|idx| entries.get(idx));
    let has_changes = delete_target_index
        .and_then(|idx| entry_summaries.get(idx).and_then(|s| s.as_ref()))
        .is_some_and(GitSummary::has_changes);
    let message = if let Some(entry) = entry {
        if has_changes {
            format!(
                "{}\n\nHas uncommitted changes. Removes folder and local branch.",
                entry.folder_name
            )
        } else {
            format!(
                "{}\n\nAlready merged into main. Removes folder and local branch.",
                entry.folder_name
            )
        }
    } else {
        "Delete this worktree?".to_string()
    };
    let actions = ["Delete worktree", "Keep worktree", "Cancel"];
    let selected = delete_action_index.min(2);
    let mut y = inner.y;
    for line in message.lines() {
        let (row, next) = place_row(inner, y, 1);
        f.render_widget(
            Paragraph::new(line.to_string())
                .style(Style::default().fg(theme.foreground).bg(bg_color(theme))),
            clip_rect_to_frame(row, frame),
        );
        y = next;
    }
    y = y.saturating_add(1);
    for (idx, label) in actions.iter().enumerate() {
        let (row, next) = place_row(inner, y, 1);
        let marker = if idx == selected { "> " } else { "  " };
        let style = if idx == selected {
            Style::default()
                .fg(if idx == 0 { Color::Red } else { theme.accent })
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        f.render_widget(
            Paragraph::new(format!("{marker}{label}")).style(style),
            clip_rect_to_frame(row, frame),
        );
        y = next;
    }
}

fn render_new_worktree_form(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    name: &str,
    base: &str,
    branch: &str,
    field: NewWorktreeField,
    error: Option<&str>,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ New Worktree ─");
    let fields = [
        (NewWorktreeField::Name, "Name:", name),
        (NewWorktreeField::Base, "Base:", base),
        (NewWorktreeField::Branch, "Branch:", branch),
    ];
    let mut y = inner.y;
    for (fkind, label, value) in fields {
        let (row, next) = place_row(inner, y, 1);
        let active = fkind == field;
        let style = if active {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        f.render_widget(
            Paragraph::new(format!("{label} {value}")).style(style),
            clip_rect_to_frame(row, frame),
        );
        y = next;
    }
    if let Some(err) = error {
        let (row, _) = place_row(inner, y, 1);
        f.render_widget(
            Paragraph::new(err).style(Style::default().fg(Color::Red)),
            clip_rect_to_frame(row, frame),
        );
    }
    let (row, _) = place_row(inner, inner.bottom().saturating_sub(2), 1);
    f.render_widget(
        Paragraph::new("Enter: Create   Esc: Cancel")
            .style(Style::default().fg(theme.muted)),
        clip_rect_to_frame(row, frame),
    );
}

fn render_run_agent_form(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    task: &str,
    entries: &[WorktreeInfo],
    selected_index: usize,
    repo_root: &Path,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Run Agent ─");
    let worktree_name = selected_index
        .checked_sub(1)
        .and_then(|i| entries.get(i))
        .map(|e| worktree_display_name(e, repo_root))
        .unwrap_or_else(|| "worktree".to_string());
    let mut y = inner.y;
    let lines = [
        format!("Worktree: {worktree_name}"),
        String::new(),
        "Task:".to_string(),
        task.to_string(),
        String::new(),
        "Enter: Start   Esc: Cancel".to_string(),
    ];
    for line in lines {
        let (row, next) = place_row(inner, y, 1);
        f.render_widget(
            Paragraph::new(line).style(Style::default().fg(theme.foreground)),
            clip_rect_to_frame(row, frame),
        );
        y = next;
    }
}

fn render_commit_review(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    files: &[ChangedFile],
    scroll: usize,
    file_cursor: usize,
    select_all_tracked: bool,
    select_all_untracked: bool,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Commit ─");
    let selected = files.iter().filter(|f| f.selected).count();
    let mut lines = vec![
        Line::from(format!("{selected} files selected")),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("[{}] Select all tracked", if select_all_tracked { 'x' } else { ' ' }),
                Style::default().fg(theme.muted),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!(
                "[{}] Select all untracked",
                if select_all_untracked { 'x' } else { ' ' }
            ),
            Style::default().fg(theme.muted),
        )]),
        Line::from(""),
    ];

    let visible: Vec<_> = files.iter().enumerate().skip(scroll).take(12).collect();
    for (idx, file) in visible {
        let marker = if idx == file_cursor { '>' } else { ' ' };
        let check = if file.selected { 'x' } else { ' ' };
        let kind = match file.kind {
            FileChangeKind::Modified => "Modified",
            FileChangeKind::Added => "Added",
            FileChangeKind::Deleted => "Deleted",
            FileChangeKind::Renamed => "Renamed",
            FileChangeKind::Untracked => "Added",
        };
        let large = if file.large { "   large" } else { "" };
        let style = if file.large {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(theme.foreground)
        };
        lines.push(Line::from(vec![Span::styled(
            format!("[{check}] {kind} {}{large}", file.path),
            style,
        )]));
        let _ = marker;
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter: Commit selected   d: diff   Esc: Cancel",
        Style::default().fg(theme.muted),
    )));

    let list_area = inner;
    render_lines_in_area(
        f,
        list_area,
        lines,
        Style::default().fg(theme.foreground).bg(bg_color(theme)),
        frame,
    );
}

fn render_commit_message(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    _theme: Theme,
    message: &str,
    selected_count: usize,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Commit Message ─");
    let mut y = inner.y;
    let header = format!("Commit selected files\n\n{selected_count} files selected\n");
    for line in header.lines() {
        let (row, next) = place_row(inner, y, 1);
        f.render_widget(Paragraph::new(line.to_string()), clip_rect_to_frame(row, frame));
        y = next;
    }
    let (box_area, next) = place_row(inner, y, 4);
    let block = Block::default().borders(Borders::ALL);
    let msg_inner = block.inner(box_area);
    f.render_widget(block, clip_rect_to_frame(box_area, frame));
    f.render_widget(
        Paragraph::new(message).wrap(Wrap { trim: false }),
        clip_rect_to_frame(msg_inner, frame),
    );
    let (row, _) = place_row(inner, next, 1);
    f.render_widget(
        Paragraph::new("Enter: Commit   Esc: Back"),
        clip_rect_to_frame(row, frame),
    );
}

fn render_merge_confirm(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    _theme: Theme,
    entries: &[WorktreeInfo],
    entry_summaries: &[Option<GitSummary>],
    entry_snapshots: &[WorktreeGitSnapshot],
    entry_index: usize,
    repo_root: &Path,
    target_branch: &str,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Merge Worktree ─");
    let entry = entries.get(entry_index);
    let snapshot = entry_snapshots.get(entry_index);
    let clean = entry_summaries
        .get(entry_index)
        .and_then(|s| s.as_ref())
        .is_none_or(|s| !s.has_changes());
    let name = entry
        .map(|e| worktree_display_name(e, repo_root))
        .unwrap_or_default();
    let branch = entry.map(|e| e.branch.as_str()).unwrap_or("");
    let ahead = snapshot.map(|s| s.ahead).unwrap_or(0);
    let lines = vec![
        name,
        format!("branch: {branch}"),
        format!("target: {target_branch}"),
        format!("{ahead} commits ahead"),
        if clean {
            "working tree clean".to_string()
        } else {
            "working tree dirty".to_string()
        },
        "checks passed".to_string(),
        "merge dry-run ok".to_string(),
        String::new(),
        "Enter: Merge into main".to_string(),
        "Esc: Cancel".to_string(),
    ];
    let mut y = inner.y;
    for line in lines {
        let (row, next) = place_row(inner, y, 1);
        f.render_widget(Paragraph::new(line), clip_rect_to_frame(row, frame));
        y = next;
    }
}

fn render_checks_failed(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    theme: Theme,
    entries: &[WorktreeInfo],
    entry_index: usize,
    repo_root: &Path,
    results: &CheckResults,
) {
    let frame = f.size();
    let inner = worktree_picker_inner(area, "─ Checks Failed ─");
    let name = entries
        .get(entry_index)
        .map(|e| worktree_display_name(e, repo_root))
        .unwrap_or_default();
    fn mark(ok: Option<bool>) -> &'static str {
        match ok {
            Some(true) => "passed",
            Some(false) => "failed",
            None => "skipped",
        }
    }
    let lines = vec![
        format!("Checks failed: {name}"),
        String::new(),
        format!("lint        {}", mark(results.lint_ok)),
        format!("tests       {}", mark(results.tests_ok)),
        format!("typecheck   {}", mark(results.typecheck_ok)),
        String::new(),
        results.detail.clone(),
        String::new(),
        "Enter: Re-run checks   Esc: Cancel".to_string(),
    ];
    let mut y = inner.y;
    for line in lines {
        let (row, next) = place_row(inner, y, 1);
        f.render_widget(
            Paragraph::new(line).style(Style::default().fg(theme.foreground)),
            clip_rect_to_frame(row, frame),
        );
        y = next;
    }
}

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

fn clip_rect_to_frame(area: Rect, frame: Rect) -> Rect {
    crate::layout::clip_rect_to_frame(area, frame)
}

pub(crate) fn suggested_commit_message(files: &[ChangedFile]) -> String {
    let paths: Vec<_> = files
        .iter()
        .filter(|f| f.selected)
        .map(|f| f.path.as_str())
        .take(5)
        .collect();
    if paths.is_empty() {
        return "Update worktree changes".to_string();
    }
    if paths.len() == 1 {
        return format!("Update {}", paths[0]);
    }
    format!("Update {} files", paths.len())
}

pub(crate) fn action_for_row(
    state: WorktreeLifecycleState,
) -> Option<WorktreeAction> {
    let action = state.action();
    if action == WorktreeAction::None {
        None
    } else {
        Some(action)
    }
}
