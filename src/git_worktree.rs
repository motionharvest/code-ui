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

pub(crate) fn create_worktree(git_root: &Path, branch: &str) -> anyhow::Result<PathBuf> {
    ensure_codeui_gitignored(git_root)?;
    let worktree_path = next_available_worktree_path(git_root, branch);

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
            branch,
            path,
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to create worktree");
    }
    Ok(worktree_path)
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
