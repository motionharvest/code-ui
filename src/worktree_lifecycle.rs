use std::path::Path;

use crate::git_status::GitSummary;
use crate::git_worktree::WorktreeGitSnapshot;

/// Visible lifecycle state for a worktree row (what is true right now).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeLifecycleState {
    Current,
    Clean,
    Dirty,
    Ahead,
    Behind,
    Diverged,
    Conflicted,
    ChecksFailed,
    MergeReady,
    Merged,
    Stale,
    Running,
    Checking,
}

/// Focused action when the status column is selected (what you can do next).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeAction {
    None,
    NewTask,
    RunAgent,
    Commit,
    PrepareMerge,
    Rebase,
    Reconcile,
    Resolve,
    Inspect,
    Merge,
    Delete,
    Refresh,
}

/// Transient UI/runtime flags not inferable from git alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeRuntimeFlag {
    Running,
    Checking,
    ChecksFailed,
}

impl WorktreeLifecycleState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Clean => "clean",
            Self::Dirty => "dirty",
            Self::Ahead => "ahead",
            Self::Behind => "behind",
            Self::Diverged => "diverged",
            Self::Conflicted => "conflicted",
            Self::ChecksFailed => "checks failed",
            Self::MergeReady => "merge ready",
            Self::Merged => "merged",
            Self::Stale => "stale",
            Self::Running => "running",
            Self::Checking => "checking",
        }
    }

    pub(crate) fn action(self) -> WorktreeAction {
        match self {
            Self::Current => WorktreeAction::NewTask,
            Self::Clean => WorktreeAction::RunAgent,
            Self::Dirty => WorktreeAction::Commit,
            Self::Ahead => WorktreeAction::PrepareMerge,
            Self::Behind => WorktreeAction::Rebase,
            Self::Diverged => WorktreeAction::Reconcile,
            Self::Conflicted => WorktreeAction::Resolve,
            Self::ChecksFailed => WorktreeAction::Inspect,
            Self::MergeReady => WorktreeAction::Merge,
            Self::Merged => WorktreeAction::Delete,
            Self::Stale => WorktreeAction::Refresh,
            Self::Running | Self::Checking => WorktreeAction::None,
        }
    }

    pub(crate) fn action_label(self) -> Option<&'static str> {
        let action = self.action();
        if action == WorktreeAction::None {
            return None;
        }
        Some(action.label())
    }
}

impl WorktreeAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::NewTask => "new task",
            Self::RunAgent => "run agent",
            Self::Commit => "commit",
            Self::PrepareMerge => "prepare merge",
            Self::Rebase => "rebase",
            Self::Reconcile => "reconcile",
            Self::Resolve => "resolve",
            Self::Inspect => "inspect",
            Self::Merge => "merge",
            Self::Delete => "delete",
            Self::Refresh => "refresh",
        }
    }
}

pub(crate) fn compute_worktree_state(
    is_current: bool,
    summary: Option<&GitSummary>,
    snapshot: &WorktreeGitSnapshot,
    runtime: Option<WorktreeRuntimeFlag>,
) -> WorktreeLifecycleState {
    if let Some(flag) = runtime {
        return match flag {
            WorktreeRuntimeFlag::Running => WorktreeLifecycleState::Running,
            WorktreeRuntimeFlag::Checking => WorktreeLifecycleState::Checking,
            WorktreeRuntimeFlag::ChecksFailed => WorktreeLifecycleState::ChecksFailed,
        };
    }

    if is_current {
        return WorktreeLifecycleState::Current;
    }

    if snapshot.merged_into_target {
        return WorktreeLifecycleState::Merged;
    }

    if snapshot.merge_conflicted || snapshot.rebase_in_progress {
        return WorktreeLifecycleState::Conflicted;
    }

    if summary.is_some_and(GitSummary::has_changes) {
        return WorktreeLifecycleState::Dirty;
    }

    if snapshot.ahead > 0 && snapshot.behind > 0 {
        return WorktreeLifecycleState::Diverged;
    }

    if snapshot.behind > 0 {
        return WorktreeLifecycleState::Behind;
    }

    if snapshot.merge_ready {
        return WorktreeLifecycleState::MergeReady;
    }

    if snapshot.ahead > 0 {
        return WorktreeLifecycleState::Ahead;
    }

    WorktreeLifecycleState::Clean
}

pub(crate) fn agent_branch_name(slug: &str) -> String {
    let slug = slug.trim().trim_matches('/');
    if slug.is_empty() {
        "agent/worktree".to_string()
    } else if slug.starts_with("agent/") {
        slug.to_string()
    } else {
        format!("agent/{slug}")
    }
}

pub(crate) fn worktree_display_name(entry: &crate::git_worktree::WorktreeInfo, repo_root: &Path) -> String {
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{repo_name}-");
    if entry.folder_name.starts_with(&prefix) {
        entry.folder_name[prefix.len()..].to_string()
    } else {
        entry.folder_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_worktree::WorktreeGitSnapshot;

    fn snapshot_clean_ahead() -> WorktreeGitSnapshot {
        WorktreeGitSnapshot {
            ahead: 2,
            behind: 0,
            merged_into_target: false,
            merge_conflicted: false,
            rebase_in_progress: false,
            merge_ready: true,
            merge_dry_run_ok: true,
        }
    }

    #[test]
    fn dirty_beats_ahead() {
        let summary = GitSummary {
            branch: "feat".to_string(),
            staged: 0,
            unstaged: 1,
            untracked: 0,
            on_github: false,
        };
        let state = compute_worktree_state(false, Some(&summary), &snapshot_clean_ahead(), None);
        assert_eq!(state, WorktreeLifecycleState::Dirty);
        assert_eq!(state.action(), WorktreeAction::Commit);
    }

    #[test]
    fn runtime_running_overrides_clean() {
        let state = compute_worktree_state(
            false,
            None,
            &WorktreeGitSnapshot::default(),
            Some(WorktreeRuntimeFlag::Running),
        );
        assert_eq!(state, WorktreeLifecycleState::Running);
    }

    #[test]
    fn agent_branch_name_prefixes_slug() {
        assert_eq!(agent_branch_name("theme-pass"), "agent/theme-pass");
        assert_eq!(agent_branch_name("agent/x"), "agent/x");
    }
}
