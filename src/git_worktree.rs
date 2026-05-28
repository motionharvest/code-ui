use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub folder_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct WorktreeGitSnapshot {
    pub ahead: usize,
    pub behind: usize,
    pub merged_into_target: bool,
    pub merge_conflicted: bool,
    pub rebase_in_progress: bool,
    pub merge_ready: bool,
    pub merge_dry_run_ok: bool,
}

const LARGE_FILE_BYTES: u64 = 5 * 1024 * 1024;

pub(crate) fn git_root(cwd: &Path) -> Option<PathBuf> {
    let cwd = cwd.to_str()?;
    let output = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    Some(PathBuf::from(root))
}

/// Leaf directory name for pane subtitles: git worktree/repo root when `path` is inside a repo.
pub(crate) fn display_folder_name(path: &Path) -> Option<String> {
    let root = git_root(path).unwrap_or_else(|| path.to_path_buf());
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

/// Relative path below the git toplevel, e.g. `src/ui` or `.pi`. Empty when cwd is the root.
pub(crate) fn relative_subpath_from_git_root(cwd: &Path) -> Option<String> {
    let root = git_root(cwd)?;
    if paths_equal(cwd, &root) {
        return None;
    }

    if let Ok(relative) = cwd.strip_prefix(&root) {
        return subpath_display(relative);
    }

    let canonical_root = fs::canonicalize(&root).ok()?;
    let canonical_cwd = fs::canonicalize(cwd).ok()?;
    if paths_equal(&canonical_cwd, &canonical_root) {
        return None;
    }
    canonical_cwd
        .strip_prefix(&canonical_root)
        .ok()
        .and_then(subpath_display)
}

fn subpath_display(relative: &Path) -> Option<String> {
    let relative = relative.to_string_lossy();
    if relative.is_empty() || relative == "." {
        return None;
    }
    Some(relative.replace('\\', "/"))
}

pub(crate) fn list_worktrees(git_root: &Path) -> Vec<WorktreeInfo> {
    let git_root = git_root.to_str().unwrap_or("");
    let output = match Command::new("git")
        .args(["-C", git_root, "worktree", "list", "--porcelain"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    let mut path = None;
    let mut branch = None;

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            branch = None;
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(short_branch_name(value));
        } else if line.is_empty() {
            if let Some(path) = path.take() {
                let branch = branch.take().unwrap_or_else(|| "detached".to_string());
                let folder_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| branch.clone());
                entries.push(WorktreeInfo {
                    path,
                    branch,
                    folder_name,
                });
            }
        }
    }

    if let Some(path) = path {
        let branch = branch.unwrap_or_else(|| "detached".to_string());
        let folder_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| branch.clone());
        entries.push(WorktreeInfo {
            path,
            branch,
            folder_name,
        });
    }

    entries
}

fn short_branch_name(raw: &str) -> String {
    raw.strip_prefix("refs/heads/")
        .unwrap_or(raw)
        .to_string()
}

pub(crate) fn branch_base_from_pane_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed
    }
}

pub(crate) fn next_available_branch(git_root: &Path, base: &str) -> String {
    if !branch_exists(git_root, base) {
        return base.to_string();
    }
    for n in 1..1000 {
        let candidate = format!("{base}-{n}");
        if !branch_exists(git_root, &candidate) {
            return candidate;
        }
    }
    format!("{base}-extra")
}

fn branch_exists(git_root: &Path, branch: &str) -> bool {
    let Some(root) = git_root.to_str() else {
        return false;
    };
    Command::new("git")
        .args([
            "-C",
            root,
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn sibling_worktree_path(git_root: &Path, branch: &str) -> PathBuf {
    let parent = git_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".."));
    let repo_name = git_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let branch_suffix = branch.replace('/', "-");
    parent.join(format!("{repo_name}-{branch_suffix}"))
}

pub(crate) fn next_available_worktree_path(git_root: &Path, branch: &str) -> PathBuf {
    let base = sibling_worktree_path(git_root, branch);
    if !base.exists() {
        return base;
    }
    for n in 1..1000 {
        let candidate = {
            let parent = base.parent().unwrap_or_else(|| Path::new(".."));
            let stem = base
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("worktree");
            parent.join(format!("{stem}-{n}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

pub(crate) fn ensure_codeui_gitignored(git_root: &Path) -> anyhow::Result<()> {
    let gitignore = git_root.join(".gitignore");
    let markers = [".codeui", ".codeui/", ".codeui/*"];
    if gitignore.exists() {
        let content = fs::read_to_string(&gitignore)?;
        if content.lines().any(|line| {
            let trimmed = line.trim();
            markers.contains(&trimmed)
        }) {
            return Ok(());
        }
        let mut updated = content;
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(".codeui/\n");
        fs::write(gitignore, updated)?;
    } else {
        fs::write(gitignore, ".codeui/\n")?;
    }
    Ok(())
}

pub(crate) fn default_integration_branch(git_root: &Path) -> String {
    for name in ["main", "master"] {
        if branch_exists(git_root, name) {
            return name.to_string();
        }
    }
    "main".to_string()
}

pub(crate) fn sibling_worktree_path_for_name(git_root: &Path, name: &str) -> PathBuf {
    let parent = git_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".."));
    let repo_name = git_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let slug = name.trim().replace('/', "-");
    parent.join(format!("{repo_name}-{slug}"))
}

pub(crate) fn next_available_worktree_path_for_name(git_root: &Path, name: &str) -> PathBuf {
    let base = sibling_worktree_path_for_name(git_root, name);
    if !base.exists() {
        return base;
    }
    for n in 1..1000 {
        let candidate = {
            let parent = base.parent().unwrap_or_else(|| Path::new(".."));
            let stem = base
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("worktree");
            parent.join(format!("{stem}-{n}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

pub(crate) fn ahead_behind(git_root: &Path, branch: &str, target: &str) -> (usize, usize) {
    let Some(root) = git_root.to_str() else {
        return (0, 0);
    };
    if branch == target {
        return (0, 0);
    }
    let output = Command::new("git")
        .args([
            "-C",
            root,
            "rev-list",
            "--left-right",
            "--count",
            &format!("{target}...{branch}"),
        ])
        .output();
    let Ok(output) = output else {
        return (0, 0);
    };
    if !output.status.success() {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (ahead, behind)
}

pub(crate) fn branch_merged_into(git_root: &Path, branch: &str, target: &str) -> bool {
    if branch == target {
        return false;
    }
    let Some(root) = git_root.to_str() else {
        return false;
    };
    let output = Command::new("git")
        .args(["-C", root, "branch", "--merged", target])
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .map(|line| line.strip_prefix('*').unwrap_or(line).trim())
        .any(|line| line == branch)
}

fn git_path_exists(worktree_path: &Path, name: &str) -> bool {
    let Some(cwd) = worktree_path.to_str() else {
        return false;
    };
    let Some(output) = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--git-path", name])
        .output()
        .ok()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    !path.is_empty() && Path::new(&path).exists()
}

pub(crate) fn merge_in_progress(worktree_path: &Path) -> bool {
    git_path_exists(worktree_path, "MERGE_HEAD")
}

pub(crate) fn rebase_in_progress(worktree_path: &Path) -> bool {
    git_path_exists(worktree_path, "rebase-merge")
        || git_path_exists(worktree_path, "rebase-apply")
}

pub(crate) fn merge_conflicted(worktree_path: &Path) -> bool {
    let Some(cwd) = worktree_path.to_str() else {
        return false;
    };
    let output = Command::new("git")
        .args(["-C", cwd, "diff", "--name-only", "--diff-filter=U"])
        .output();
    let Ok(output) = output else {
        return false;
    };
    output.status.success() && !output.stdout.is_empty()
}

pub(crate) fn merge_dry_run_ok(git_root: &Path, branch: &str, target: &str) -> bool {
    if branch == target {
        return true;
    }
    let Some(root) = git_root.to_str() else {
        return false;
    };
    let output = Command::new("git")
        .args([
            "-C",
            root,
            "merge-tree",
            "--merge-base",
            target,
            branch,
        ])
        .output();
    let Ok(output) = output else {
        return true;
    };
    output.status.success()
}

pub(crate) fn worktree_git_snapshot(
    git_root: &Path,
    worktree_path: &Path,
    branch: &str,
    target: &str,
    clean: bool,
    checks_passed: Option<bool>,
) -> WorktreeGitSnapshot {
    let (ahead, behind) = ahead_behind(git_root, branch, target);
    let merged_into_target = branch_merged_into(git_root, branch, target);
    let rebase = rebase_in_progress(worktree_path);
    let conflicted = merge_conflicted(worktree_path) || merge_in_progress(worktree_path);
    let merge_dry_run_ok = merge_dry_run_ok(git_root, branch, target);
    let checks_ok = checks_passed.unwrap_or(true);
    let merge_ready = clean
        && ahead > 0
        && behind == 0
        && merge_dry_run_ok
        && checks_ok
        && !merged_into_target
        && !conflicted;

    WorktreeGitSnapshot {
        ahead,
        behind,
        merged_into_target,
        merge_conflicted: conflicted,
        rebase_in_progress: rebase,
        merge_ready,
        merge_dry_run_ok,
    }
}

pub(crate) fn create_worktree(git_root: &Path, branch: &str) -> anyhow::Result<PathBuf> {
    create_worktree_from_base(git_root, branch, branch, branch)
}

pub(crate) fn create_worktree_from_base(
    git_root: &Path,
    name: &str,
    base_branch: &str,
    new_branch: &str,
) -> anyhow::Result<PathBuf> {
    ensure_codeui_gitignored(git_root)?;
    let worktree_path = next_available_worktree_path_for_name(git_root, name);

    let Some(root) = git_root.to_str() else {
        anyhow::bail!("invalid git root path");
    };
    let Some(path) = worktree_path.to_str() else {
        anyhow::bail!("invalid worktree path");
    };

    let status = Command::new("git")
        .args([
            "-C",
            root,
            "worktree",
            "add",
            "-b",
            new_branch,
            path,
            base_branch,
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to create worktree");
    }
    Ok(worktree_path)
}

pub(crate) fn merge_branch_into(
    git_root: &Path,
    worktree_path: &Path,
    branch: &str,
    target: &str,
) -> anyhow::Result<()> {
    let Some(root) = git_root.to_str() else {
        anyhow::bail!("invalid git root path");
    };
    let status = Command::new("git")
        .args(["-C", root, "checkout", target])
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to checkout {target}");
    }
    let status = Command::new("git")
        .args(["-C", root, "merge", "--no-ff", branch])
        .status()?;
    if !status.success() {
        let _ = Command::new("git")
            .args(["-C", root, "merge", "--abort"])
            .status();
        anyhow::bail!("merge failed");
    }
    let Some(path) = worktree_path.to_str() else {
        return Ok(());
    };
    let _ = Command::new("git")
        .args(["-C", path, "checkout", branch])
        .status();
    Ok(())
}

pub(crate) fn delete_local_branch(git_root: &Path, branch: &str, force: bool) -> anyhow::Result<()> {
    let Some(root) = git_root.to_str() else {
        anyhow::bail!("invalid git root path");
    };
    let mut args = vec!["-C", root, "branch"];
    if force {
        args.push("-D");
    } else {
        args.push("-d");
    }
    args.push(branch);
    let status = Command::new("git").args(args).status()?;
    if !status.success() {
        anyhow::bail!("failed to delete branch {branch}");
    }
    Ok(())
}

pub(crate) fn is_large_untracked_path(worktree_path: &Path, rel_path: &str) -> bool {
    let path = worktree_path.join(rel_path);
    path.metadata()
        .ok()
        .map(|meta| meta.len() >= LARGE_FILE_BYTES)
        .unwrap_or(false)
}

pub(crate) fn delete_worktree(
    git_root: &Path,
    worktree_path: &Path,
    force: bool,
) -> anyhow::Result<()> {
    let Some(root) = git_root.to_str() else {
        anyhow::bail!("invalid git root path");
    };
    let Some(path) = worktree_path.to_str() else {
        anyhow::bail!("invalid worktree path");
    };

    let mut args = vec!["-C", root, "worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);

    let status = Command::new("git").args(args).status()?;
    if !status.success() {
        anyhow::bail!("failed to remove worktree");
    }
    Ok(())
}

pub(crate) fn worktree_is_deletable(
    entry: &WorktreeInfo,
    repo_root: &Path,
    current_path: Option<&Path>,
) -> bool {
    if paths_equal(&entry.path, repo_root) {
        return false;
    }
    if current_path.is_some_and(|path| paths_equal(path, &entry.path)) {
        return false;
    }
    true
}

pub(crate) fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    fs::canonicalize(a)
        .ok()
        .zip(fs::canonicalize(b).ok())
        .is_some_and(|(left, right)| left == right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_subpath_from_git_root_returns_path_below_toplevel() {
        let repo_root = std::env::current_dir().expect("cwd");
        let nested = repo_root.join("src");
        if !nested.is_dir() {
            return;
        }
        assert_eq!(
            relative_subpath_from_git_root(&nested).as_deref(),
            Some("src")
        );
        assert_eq!(relative_subpath_from_git_root(&repo_root), None);
    }

    #[test]
    fn display_folder_name_uses_git_toplevel_not_cwd_leaf() {
        let repo_root = std::env::current_dir().expect("cwd");
        let nested = repo_root.join("src");
        if !nested.is_dir() {
            return;
        }
        let name = display_folder_name(&nested).expect("name");
        assert_eq!(
            name,
            repo_root
                .file_name()
                .expect("repo root name")
                .to_string_lossy()
        );
    }

    #[test]
    fn branch_base_from_pane_name_sanitizes() {
        assert_eq!(branch_base_from_pane_name("Pane 33"), "pane-33");
        assert_eq!(branch_base_from_pane_name("  "), "worktree");
    }

    #[test]
    fn short_branch_name_strips_refs_heads() {
        assert_eq!(
            short_branch_name("refs/heads/feat/pane-summary"),
            "feat/pane-summary"
        );
    }

    #[test]
    fn sibling_worktree_path_is_outside_repo_folder() {
        let root = PathBuf::from("/home/aaron/lab/code-ui");
        assert_eq!(
            sibling_worktree_path(&root, "pane-43"),
            PathBuf::from("/home/aaron/lab/code-ui-pane-43")
        );
    }

    #[test]
    fn sibling_worktree_path_sanitizes_branch_slashes() {
        let root = PathBuf::from("/home/aaron/lab/code-ui");
        assert_eq!(
            sibling_worktree_path(&root, "feat/pane-summary"),
            PathBuf::from("/home/aaron/lab/code-ui-feat-pane-summary")
        );
    }
}
