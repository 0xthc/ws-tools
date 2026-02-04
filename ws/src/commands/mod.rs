mod ai;
mod config;
mod doctor;
mod git_workflow;
mod layout;
mod status;
mod update;
mod workspace;

pub use ai::ai;
pub use config::{config, init};
pub use doctor::doctor;
pub use git_workflow::{clone_repo, gc, pr_create, pr_list, review};
pub use layout::layout;
pub use status::{dashboard, status, StatusAction};
pub use update::update;
pub use workspace::{delete, new, open, reload, select, sync};

use crate::git;
use anyhow::{Context, Result};
use std::path::Path;

/// Generate session name from directory
pub(crate) fn get_session_name(dir: &Path) -> Result<String> {
    let repo_name = dir
        .file_name()
        .context("Invalid directory")?
        .to_string_lossy();
    let branch = git::get_branch(dir)?;
    let branch_safe = git::sanitize_branch(&branch);
    Ok(format!("{}-{}", repo_name, branch_safe))
}

/// Generate window title for Ghostty tab: "repo/worktree [branch]"
pub(crate) fn get_window_title(dir: &Path) -> Result<String> {
    let worktree_name = dir
        .file_name()
        .context("Invalid directory")?
        .to_string_lossy();

    // Try to get the repo name from git root
    let git_root = git::get_root(Some(dir)).ok();
    let repo_name = git_root
        .as_ref()
        .and_then(|r| r.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| worktree_name.to_string());

    let branch = git::get_branch(dir)?;

    // Format: "repo/worktree [branch]" or just "repo [branch]" if same
    if repo_name == worktree_name {
        Ok(format!("{} [{}]", repo_name, branch))
    } else {
        Ok(format!("{}/{} [{}]", repo_name, worktree_name, branch))
    }
}

/// Get the workspaces directory (~/.ws/workspaces/)
pub(crate) fn get_workspaces_dir() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let ws_dir = home.join(".ws").join("workspaces");

    // Create directory if it doesn't exist
    if !ws_dir.exists() {
        std::fs::create_dir_all(&ws_dir).context("Failed to create workspaces directory")?;
    }

    Ok(ws_dir)
}
