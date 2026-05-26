use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use ratatui::style::Color;

use crate::pane::Pane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitSummary {
    pub branch: String,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub on_github: bool,
}

/// Nerd Font: git repo badge icon
pub(crate) const NF_TREE: char = '\u{f418}';
/// Nerd Font: folder when cwd is not a git repository
pub(crate) const NF_FOLDER: char = '\u{e5ff}';

/// Matches `parse_git_prompt` in ~/.zshrc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitPromptKind {
    Clean,
    StagedOnly,
    UnstagedOnly,
    Mixed,
}

impl GitSummary {
    pub(crate) fn dirty_count(&self) -> usize {
        self.staged + self.unstaged + self.untracked
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.dirty_count() > 0
    }

    /// Same rules as `parse_git_prompt` in ~/.zshrc.
    pub(crate) fn prompt_kind(&self) -> GitPromptKind {
        if self.staged == 0 && self.unstaged == 0 && self.untracked == 0 {
            GitPromptKind::Clean
        } else if self.staged > 0 && self.unstaged == 0 && self.untracked == 0 {
            GitPromptKind::StagedOnly
        } else if self.staged == 0 && (self.unstaged > 0 || self.untracked > 0) {
            GitPromptKind::UnstagedOnly
        } else {
            GitPromptKind::Mixed
        }
    }

    pub(crate) fn prompt_symbol(&self) -> char {
        match self.prompt_kind() {
            GitPromptKind::Clean => '✓',
            GitPromptKind::StagedOnly => '+',
            GitPromptKind::UnstagedOnly => '!',
            GitPromptKind::Mixed => '±',
        }
    }

    pub(crate) fn prompt_color(&self) -> Color {
        match self.prompt_kind() {
            GitPromptKind::Clean => Color::Green,
            GitPromptKind::StagedOnly => Color::Blue,
            GitPromptKind::UnstagedOnly => Color::Red,
            GitPromptKind::Mixed => Color::Yellow,
        }
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
    body.push_str(&git_subtitle_tail(summary));
    body
}

fn git_subtitle_tail(summary: &GitSummary) -> String {
    format!(
        " ({branch} {symbol})",
        branch = summary.branch,
        symbol = summary.prompt_symbol()
    )
}

fn format_subpath_tail(subpath: Option<&str>) -> String {
    subpath
        .filter(|path| !path.is_empty())
        .map(|path| format!(" ./{path}"))
        .unwrap_or_default()
}

pub(crate) fn pane_subtitle_parts(
    folder_name: &str,
    summary: Option<&GitSummary>,
    subpath: Option<&str>,
    max_cols: usize,
) -> Option<(char, String, String, Option<Color>, String)> {
    if max_cols == 0 || folder_name.is_empty() {
        return None;
    }

    let icon = match summary {
        Some(_) => NF_TREE,
        None => NF_FOLDER,
    };
    let branch_tail = summary.map(git_subtitle_tail).unwrap_or_default();
    let subpath_tail = format_subpath_tail(subpath);
    let git_color = summary.map(GitSummary::prompt_color);

    let body = format!("{icon} {folder_name}{branch_tail}{subpath_tail}");
    let body_len = body.chars().count();

    if body_len <= max_cols {
        return Some((
            icon,
            folder_name.to_string(),
            branch_tail,
            git_color,
            subpath_tail,
        ));
    }

    if !subpath_tail.is_empty() {
        let without_subpath = format!("{icon} {folder_name}{branch_tail}");
        if without_subpath.chars().count() <= max_cols {
            return Some((
                icon,
                folder_name.to_string(),
                branch_tail,
                git_color,
                String::new(),
            ));
        }
    }

    truncate_pane_subtitle_without_subpath(
        icon,
        folder_name,
        branch_tail,
        git_color,
        max_cols,
    )
}

fn truncate_pane_subtitle_without_subpath(
    icon: char,
    folder_name: &str,
    branch_tail: String,
    git_color: Option<Color>,
    max_cols: usize,
) -> Option<(char, String, String, Option<Color>, String)> {
    let body = format!("{icon} {folder_name}{branch_tail}");
    let body_len = body.chars().count();

    if body_len <= max_cols {
        return Some((
            icon,
            folder_name.to_string(),
            branch_tail,
            git_color,
            String::new(),
        ));
    }

    let visible_len = if max_cols <= 1 {
        max_cols
    } else {
        max_cols - 1
    };
    let has_ellipsis = max_cols > 1;
    let name_len = folder_name.chars().count();
    let name_end = 2 + name_len;

    if visible_len <= 2 {
        return Some((icon, String::new(), String::new(), git_color, String::new()));
    }

    if visible_len <= name_end {
        let mut name: String = body.chars().skip(2).take(visible_len - 2).collect();
        if has_ellipsis {
            name.push('…');
        }
        return Some((icon, name, String::new(), git_color, String::new()));
    }

    let mut visible_tail: String = branch_tail.chars().take(visible_len - name_end).collect();
    if has_ellipsis && visible_len < body_len {
        visible_tail.push('…');
    }
    Some((
        icon,
        folder_name.to_string(),
        visible_tail,
        git_color,
        String::new(),
    ))
}

pub(crate) fn truncate_pane_subtitle_body(
    folder_name: &str,
    summary: Option<&GitSummary>,
    subpath: Option<&str>,
    max_cols: usize,
) -> String {
    pane_subtitle_parts(folder_name, summary, subpath, max_cols)
        .map(|(icon, name, branch_tail, _, subpath_tail)| {
            format!("{icon} {name}{branch_tail}{subpath_tail}")
        })
        .unwrap_or_default()
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
        self.pane_path(pane)
            .and_then(|path| crate::git_worktree::display_folder_name(&path))
    }

    pub(crate) fn subpath_for_pane(&mut self, pane: &Pane) -> Option<String> {
        self.pane_path(pane)
            .and_then(|path| crate::git_worktree::relative_subpath_from_git_root(&path))
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
    fn git_badge_body_starts_with_tree_and_paren_prompt() {
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
        assert!(body.ends_with("(master !)"));
    }

    #[test]
    fn prompt_kind_matches_zshrc_rules() {
        let clean = GitSummary {
            branch: "main".to_string(),
            staged: 0,
            unstaged: 0,
            untracked: 0,
            on_github: false,
        };
        assert_eq!(clean.prompt_kind(), GitPromptKind::Clean);
        assert_eq!(clean.prompt_symbol(), '✓');
        assert_eq!(clean.prompt_color(), Color::Green);

        let staged_only = GitSummary {
            staged: 1,
            ..clean.clone()
        };
        assert_eq!(staged_only.prompt_kind(), GitPromptKind::StagedOnly);
        assert_eq!(staged_only.prompt_symbol(), '+');
        assert_eq!(staged_only.prompt_color(), Color::Blue);

        let unstaged_only = GitSummary {
            unstaged: 1,
            ..clean.clone()
        };
        assert_eq!(unstaged_only.prompt_kind(), GitPromptKind::UnstagedOnly);
        assert_eq!(unstaged_only.prompt_symbol(), '!');
        assert_eq!(unstaged_only.prompt_color(), Color::Red);

        let mixed = GitSummary {
            staged: 1,
            unstaged: 1,
            ..clean
        };
        assert_eq!(mixed.prompt_kind(), GitPromptKind::Mixed);
        assert_eq!(mixed.prompt_symbol(), '±');
        assert_eq!(mixed.prompt_color(), Color::Yellow);
    }

    #[test]
    fn pane_subtitle_parts_matches_truncated_body() {
        let summary = GitSummary {
            branch: "feature/long-branch-name".to_string(),
            staged: 1,
            unstaged: 2,
            untracked: 0,
            on_github: false,
        };
        for max_cols in [10, 20, 40] {
            let truncated = truncate_pane_subtitle_body(
                "code-ui-worktree",
                Some(&summary),
                None,
                max_cols,
            );
            let parts = pane_subtitle_parts("code-ui-worktree", Some(&summary), None, max_cols)
                .map(|(icon, name, branch_tail, _, subpath_tail)| {
                    format!("{icon} {name}{branch_tail}{subpath_tail}")
                })
                .unwrap_or_default();
            assert_eq!(parts, truncated, "max_cols={max_cols}");
        }
    }

    #[test]
    fn pane_subtitle_parts_appends_muted_subpath_tail() {
        let summary = GitSummary {
            branch: "main".to_string(),
            staged: 0,
            unstaged: 1,
            untracked: 0,
            on_github: false,
        };
        let parts = pane_subtitle_parts("code-ui", Some(&summary), Some(".pi"), 80)
            .expect("parts");
        assert_eq!(parts.1, "code-ui");
        assert_eq!(parts.2, " (main !)");
        assert_eq!(parts.4, " ./.pi");
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
