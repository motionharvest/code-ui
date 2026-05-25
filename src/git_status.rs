use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crate::pane::Pane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitSummary {
    pub branch: String,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub on_github: bool,
}

/// Nerd Font: nf-md-file_tree (git repos)
pub(crate) const NF_TREE: char = '\u{e21c}';
/// Nerd Font: folder when cwd is not a git repository
pub(crate) const NF_FOLDER: char = '\u{e5ff}';
/// Nerd Font: nf-fa-git_branch
pub(crate) const NF_GIT_BRANCH: char = '\u{f126}';

impl GitSummary {
    pub(crate) fn dirty_count(&self) -> usize {
        self.staged + self.unstaged + self.untracked
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.dirty_count() > 0
    }
}

pub(crate) fn format_worktree_changes(summary: &GitSummary) -> String {
    let mut out = String::new();
    if summary.unstaged > 0 {
        out.push_str(&format!(" -{}", summary.unstaged));
    }
    let additions = summary.staged + summary.untracked;
    if additions > 0 {
        out.push_str(&format!(" +{}", additions));
    }
    out
}

pub(crate) fn folder_badge_body(folder_name: &str) -> String {
    format!("{NF_FOLDER} {folder_name}")
}

pub(crate) fn git_badge_body(folder_name: &str, summary: &GitSummary) -> String {
    let mut body = format!("{NF_TREE} {folder_name}");
    body.push(' ');
    body.push(NF_GIT_BRANCH);
    body.push(' ');
    body.push_str(&summary.branch);
    if summary.dirty_count() > 0 {
        body.push_str(&format!(" *{}", summary.dirty_count()));
    }
    body
}

pub(crate) fn truncate_pane_subtitle_body(
    folder_name: &str,
    summary: Option<&GitSummary>,
    max_cols: usize,
) -> String {
    if max_cols == 0 || folder_name.is_empty() {
        return String::new();
    }
    let body = match summary {
        Some(summary) => git_badge_body(folder_name, summary),
        None => folder_badge_body(folder_name),
    };
    truncate_to_chars(&body, max_cols)
}

fn truncate_to_chars(text: &str, max_cols: usize) -> String {
    if text.chars().count() <= max_cols {
        return text.to_string();
    }
    if max_cols == 0 {
        String::new()
    } else if max_cols == 1 {
        "…".to_string()
    } else {
        let mut out: String = text.chars().take(max_cols.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub(crate) fn query_git_summary(cwd: &Path) -> Option<GitSummary> {
    let cwd = cwd.to_str()?;
    let branch_output = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !branch_output.status.success() {
        return None;
    }

    let mut branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() {
        return None;
    }
    if branch == "HEAD" {
        let sha_output = Command::new("git")
            .args(["-C", cwd, "rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        if sha_output.status.success() {
            branch = String::from_utf8_lossy(&sha_output.stdout)
                .trim()
                .to_string();
        }
    }

    let status_output = Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .output()
        .ok()?;
    if !status_output.status.success() {
        return None;
    }

    let (staged, unstaged, untracked) =
        parse_porcelain_status(&String::from_utf8_lossy(&status_output.stdout));
    let on_github = remote_is_github(cwd);

    Some(GitSummary {
        branch,
        staged,
        unstaged,
        untracked,
        on_github,
    })
}

fn remote_is_github(cwd: &str) -> bool {
    let Some(output) = Command::new("git")
        .args(["-C", cwd, "remote", "get-url", "origin"])
        .output()
        .ok()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let url = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    url.contains("github.com") || url.contains("github:")
}

fn parse_porcelain_status(output: &str) -> (usize, usize, usize) {
    let mut staged = 0usize;
    let mut unstaged = 0usize;
    let mut untracked = 0usize;

    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("??") {
            untracked += 1;
            continue;
        }
        let Some((x, y)) = line.chars().next().zip(line.chars().nth(1)) else {
            continue;
        };
        if x != ' ' {
            staged += 1;
        }
        if y != ' ' {
            unstaged += 1;
        }
    }

    (staged, unstaged, untracked)
}

pub(crate) struct GitStatusCache {
    pane_paths: std::collections::HashMap<usize, (Instant, PathBuf)>,
    git_by_path: std::collections::HashMap<String, (Instant, Option<GitSummary>)>,
    ttl: Duration,
}

impl GitStatusCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            pane_paths: std::collections::HashMap::new(),
            git_by_path: std::collections::HashMap::new(),
            ttl,
        }
    }

    pub(crate) fn summary_for_pane(&mut self, pane: &Pane) -> Option<GitSummary> {
        let path = self.pane_path(pane)?;
        self.summary_for_path(&path)
    }

    pub(crate) fn folder_name_for_pane(&mut self, pane: &Pane) -> Option<String> {
        self.pane_path(pane).and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
        })
    }

    fn pane_path(&mut self, pane: &Pane) -> Option<PathBuf> {
        let now = Instant::now();
        if let Some((fetched_at, path)) = self.pane_paths.get(&pane.id) {
            if now.duration_since(*fetched_at) < self.ttl {
                return Some(path.clone());
            }
        }

        let path = pane.tmux_pane_path()?;
        self.pane_paths.insert(pane.id, (now, path.clone()));
        Some(path)
    }

    fn summary_for_path(&mut self, path: &Path) -> Option<GitSummary> {
        let key = path.to_string_lossy().into_owned();
        let now = Instant::now();
        if let Some((fetched_at, summary)) = self.git_by_path.get(&key) {
            if now.duration_since(*fetched_at) < self.ttl {
                return summary.clone();
            }
        }

        let summary = query_git_summary(path);
        self.git_by_path.insert(key, (now, summary.clone()));
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain_counts_staged_unstaged_and_untracked() {
        let output = " M file.rs\nM  other.rs\nMM both.rs\n?? new.rs\n";
        assert_eq!(parse_porcelain_status(output), (2, 2, 1));
    }

    #[test]
    fn git_badge_body_starts_with_tree_and_folder() {
        let body = git_badge_body(
            "code-ui",
            &GitSummary {
                branch: "master".to_string(),
                staged: 0,
                unstaged: 2,
                untracked: 0,
                on_github: true,
            },
        );
        assert!(body.starts_with(&format!("{NF_TREE} code-ui ")));
        assert!(body.contains(&format!("{NF_GIT_BRANCH} master")));
        assert!(body.ends_with(" *2"));
    }

    #[test]
    fn folder_badge_body_shows_folder_icon_and_name() {
        let body = folder_badge_body("code-ui");
        assert_eq!(body, format!("{NF_FOLDER} code-ui"));
    }

    #[test]
    fn format_worktree_changes_shows_minus_and_plus_counts() {
        assert_eq!(
            format_worktree_changes(&GitSummary {
                branch: "main".to_string(),
                staged: 2,
                unstaged: 3,
                untracked: 1,
                on_github: false,
            }),
            " -3 +3"
        );
        assert_eq!(
            format_worktree_changes(&GitSummary {
                branch: "main".to_string(),
                staged: 0,
                unstaged: 0,
                untracked: 0,
                on_github: false,
            }),
            ""
        );
    }
}
