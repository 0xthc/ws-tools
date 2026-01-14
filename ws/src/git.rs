use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Get the default branch for a repository (main, master, develop, etc.)
pub fn get_default_branch(git_root: Option<&Path>) -> String {
    // Try to get from remote HEAD (most reliable for repos with remotes)
    let mut cmd = Command::new("git");
    if let Some(path) = git_root {
        cmd.current_dir(path);
    }
    cmd.args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"]);

    if let Ok(output) = cmd.output() {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Remove "origin/" prefix if present
            if let Some(name) = branch.strip_prefix("origin/") {
                return name.to_string();
            }
            if !branch.is_empty() {
                return branch;
            }
        }
    }

    // Fallback: check which common branches exist locally or remotely
    let candidates = ["main", "master", "develop"];
    for candidate in candidates {
        let mut cmd = Command::new("git");
        if let Some(path) = git_root {
            cmd.current_dir(path);
        }
        cmd.args(["rev-parse", "--verify", &format!("origin/{}", candidate)]);

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                return candidate.to_string();
            }
        }

        // Also check local branch
        let mut cmd = Command::new("git");
        if let Some(path) = git_root {
            cmd.current_dir(path);
        }
        cmd.args(["rev-parse", "--verify", candidate]);

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                return candidate.to_string();
            }
        }
    }

    // Ultimate fallback
    "main".to_string()
}

/// Information about a git worktree
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Get the git root directory for a path
pub fn get_root(path: Option<&Path>) -> Result<PathBuf> {
    let mut cmd = Command::new("git");
    if let Some(p) = path {
        cmd.current_dir(p);
    }
    cmd.args(["rev-parse", "--show-toplevel"]);

    let output = cmd.output().context("Failed to run git")?;

    if !output.status.success() {
        anyhow::bail!("Not in a git repository");
    }

    let root = String::from_utf8(output.stdout)?;
    Ok(PathBuf::from(root.trim()))
}

/// Get the current branch name for a directory
pub fn get_branch(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["branch", "--show-current"])
        .output()
        .context("Failed to get branch")?;

    let branch = String::from_utf8(output.stdout)?.trim().to_string();

    if branch.is_empty() {
        Ok("detached".to_string())
    } else {
        Ok(branch)
    }
}

/// Sanitize a branch name for use in paths/session names
pub fn sanitize_branch(branch: &str) -> String {
    branch.replace('/', "-")
}

/// List all worktrees in a repository
pub fn list_worktrees(git_root: &Path) -> Result<Vec<Worktree>> {
    let output = Command::new("git")
        .current_dir(git_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("Failed to list worktrees")?;

    let stdout = String::from_utf8(output.stdout)?;
    let mut worktrees = Vec::new();

    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
            current_head = None;
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: branch.to_string(),
                });
            }
        } else if line.is_empty() {
            // Empty line - if we have path and head but no branch, it's detached
            if let (Some(path), Some(head)) = (current_path.take(), current_head.take()) {
                worktrees.push(Worktree {
                    path,
                    branch: format!("detached:{}", &head[..8.min(head.len())]),
                });
            }
        }
    }

    // Handle last worktree if file doesn't end with empty line
    if let Some(path) = current_path {
        if let Some(head) = current_head {
            worktrees.push(Worktree {
                path,
                branch: format!("detached:{}", &head[..8.min(head.len())]),
            });
        }
    }

    Ok(worktrees)
}

/// Create a new worktree
pub fn create_worktree(
    git_root: &Path,
    branch: &str,
    base: &str,
    target_path: &Path,
) -> Result<()> {
    // Fetch the base branch first
    let _ = Command::new("git")
        .current_dir(git_root)
        .args(["fetch", "origin", base])
        .output();

    // Try to create with new branch from origin/base
    let result = Command::new("git")
        .current_dir(git_root)
        .args([
            "worktree",
            "add",
            "-b",
            branch,
            target_path.to_str().unwrap(),
            &format!("origin/{}", base),
        ])
        .output()?;

    if result.status.success() {
        return Ok(());
    }

    // Try with existing branch
    let result = Command::new("git")
        .current_dir(git_root)
        .args(["worktree", "add", target_path.to_str().unwrap(), branch])
        .output()?;

    if result.status.success() {
        return Ok(());
    }

    anyhow::bail!(
        "Failed to create worktree. Make sure '{}' exists (tried origin/{})",
        base,
        base
    );
}

/// Remove a worktree
pub fn remove_worktree(git_root: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path.to_str().unwrap());

    let result = Command::new("git")
        .current_dir(git_root)
        .args(&args)
        .output()
        .context("Failed to remove worktree")?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        anyhow::bail!("Failed to remove worktree: {}", stderr.trim());
    }

    Ok(())
}

/// Delete a local branch
pub fn delete_branch(git_root: &Path, branch: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };

    let result = Command::new("git")
        .current_dir(git_root)
        .args(["branch", flag, branch])
        .output()
        .context("Failed to delete branch")?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        // Don't fail if branch doesn't exist or is current branch
        if !stderr.contains("not found") && !stderr.contains("Cannot delete") {
            anyhow::bail!("Failed to delete branch: {}", stderr.trim());
        }
    }

    Ok(())
}

/// Find a worktree by branch name or path
pub fn find_worktree(git_root: &Path, target: &str) -> Result<Option<Worktree>> {
    let worktrees = list_worktrees(git_root)?;

    // Try to match by branch name first
    if let Some(wt) = worktrees.iter().find(|wt| wt.branch == target) {
        return Ok(Some(wt.clone()));
    }

    // Try sanitized branch name
    let sanitized = sanitize_branch(target);
    if let Some(wt) = worktrees
        .iter()
        .find(|wt| sanitize_branch(&wt.branch) == sanitized)
    {
        return Ok(Some(wt.clone()));
    }

    // Try to match by path
    let target_path = PathBuf::from(target);
    if let Some(wt) = worktrees.iter().find(|wt| wt.path == target_path) {
        return Ok(Some(wt.clone()));
    }

    // Try to match by directory name
    if let Some(wt) = worktrees
        .iter()
        .find(|wt| wt.path.file_name().map(|n| n.to_string_lossy()) == Some(target.into()))
    {
        return Ok(Some(wt.clone()));
    }

    Ok(None)
}
