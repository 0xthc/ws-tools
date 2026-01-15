use super::workspace::{new, open};
use super::{get_session_name, get_workspaces_dir};
use crate::git;
use crate::tmux;
use anyhow::{Context, Result};
use colored::*;
use std::io::{self, BufRead, Write};
use std::process::Command;

/// Quick switch to a worktree by branch name
pub fn switch(branch: &str) -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;

    // Find the worktree
    match git::find_worktree(&git_root, branch)? {
        Some(wt) => {
            println!(
                "{} Switching to worktree: {}",
                "::".blue().bold(),
                wt.branch
            );
            open(Some(wt.path.display().to_string()))
        }
        None => {
            // Worktree doesn't exist, ask to create
            let default_branch = git::get_default_branch(Some(&git_root));
            println!("{} Worktree '{}' not found.", "::".yellow().bold(), branch);
            print!("Create new worktree from {}? [Y/n]: ", default_branch);
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input.is_empty() || input == "y" || input == "yes" {
                new(branch, &default_branch)
            } else {
                anyhow::bail!("Aborted");
            }
        }
    }
}

/// Clone a repository and set up workspace structure
pub fn clone_repo(url: &str) -> Result<()> {
    // Extract repo name from URL
    let repo_name = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .context("Invalid repository URL")?;

    let workspaces_dir = get_workspaces_dir()?;
    let repo_dir = workspaces_dir.join(repo_name);

    if repo_dir.exists() {
        println!(
            "{} Repository already exists at {}",
            "::".yellow().bold(),
            repo_dir.display()
        );
        return open(Some(repo_dir.display().to_string()));
    }

    // Create repo directory
    std::fs::create_dir_all(&repo_dir).context("Failed to create repo directory")?;

    // Clone the repository
    println!("{} Cloning {}...", "::".blue().bold(), url);
    let main_dir = repo_dir.join("main");

    let result = Command::new("git")
        .args(["clone", url, main_dir.to_str().unwrap()])
        .status()
        .context("Failed to run git clone")?;

    if !result.success() {
        // Clean up on failure
        let _ = std::fs::remove_dir_all(&repo_dir);
        anyhow::bail!("Failed to clone repository");
    }

    println!("{} Cloned to {}", "::".green().bold(), main_dir.display());

    // Open the workspace
    open(Some(main_dir.display().to_string()))
}

/// Create a pull request from current worktree
pub fn pr_create() -> Result<()> {
    // Check if gh is installed
    if which::which("gh").is_err() {
        anyhow::bail!("GitHub CLI (gh) is required. Install with: brew install gh");
    }

    let git_root = git::get_root(None).context("Not in a git repository")?;

    // Run gh pr create interactively
    println!("{} Creating pull request...", "::".blue().bold());

    let status = Command::new("gh")
        .current_dir(&git_root)
        .args(["pr", "create", "--web"])
        .status()
        .context("Failed to run gh pr create")?;

    if !status.success() {
        anyhow::bail!("Failed to create pull request");
    }

    Ok(())
}

/// List PRs for branches with worktrees
pub fn pr_list() -> Result<()> {
    // Check if gh is installed
    if which::which("gh").is_err() {
        anyhow::bail!("GitHub CLI (gh) is required. Install with: brew install gh");
    }

    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;

    // Get all open PRs
    let output = Command::new("gh")
        .current_dir(&git_root)
        .args(["pr", "list", "--json", "number,title,headRefName,state,url"])
        .output()
        .context("Failed to run gh pr list")?;

    if !output.status.success() {
        anyhow::bail!("Failed to list pull requests");
    }

    let prs: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| Vec::new());

    // Build set of worktree branches
    let worktree_branches: std::collections::HashSet<String> =
        worktrees.iter().map(|wt| wt.branch.clone()).collect();

    println!("{}", "Pull Requests for Worktrees".bold());
    println!();

    let mut found = false;
    for pr in prs {
        let branch = pr["headRefName"].as_str().unwrap_or("");
        if worktree_branches.contains(branch) {
            found = true;
            let number = pr["number"].as_u64().unwrap_or(0);
            let title = pr["title"].as_str().unwrap_or("");
            let state = pr["state"].as_str().unwrap_or("");
            let url = pr["url"].as_str().unwrap_or("");

            let state_color = match state {
                "OPEN" => "●".green(),
                "MERGED" => "●".magenta(),
                "CLOSED" => "●".red(),
                _ => "○".white(),
            };

            println!("  {} #{} {}", state_color, number, title);
            println!("    {} → {}", branch.cyan(), url.dimmed());
            println!();
        }
    }

    if !found {
        println!("  {}", "No PRs found for current worktrees".dimmed());
    }

    Ok(())
}

/// Review a pull request in a new worktree
pub fn review(pr_number: u32) -> Result<()> {
    // Check if gh is installed
    if which::which("gh").is_err() {
        anyhow::bail!("GitHub CLI (gh) is required. Install with: brew install gh");
    }

    let git_root = git::get_root(None).context("Not in a git repository")?;

    // Get PR info
    println!("{} Fetching PR #{}...", "::".blue().bold(), pr_number);

    let output = Command::new("gh")
        .current_dir(&git_root)
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "headRefName,title",
        ])
        .output()
        .context("Failed to get PR info")?;

    if !output.status.success() {
        anyhow::bail!("Failed to get PR info. Make sure PR #{} exists.", pr_number);
    }

    let pr: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse PR info")?;

    let branch = pr["headRefName"]
        .as_str()
        .context("PR has no branch")?
        .to_string();
    let title = pr["title"].as_str().unwrap_or("").to_string();

    println!("{} PR: {}", "::".blue().bold(), title);
    println!("{} Branch: {}", "::".blue().bold(), branch);

    // Check if worktree already exists
    if let Some(wt) = git::find_worktree(&git_root, &branch)? {
        println!(
            "{} Worktree already exists, opening...",
            "::".yellow().bold()
        );
        return open(Some(wt.path.display().to_string()));
    }

    // Fetch the PR branch
    println!("{} Fetching PR branch...", "::".blue().bold());

    let fetch_result = Command::new("gh")
        .current_dir(&git_root)
        .args(["pr", "checkout", &pr_number.to_string(), "--detach"])
        .status();

    // We just need the ref to exist, checkout to detach is fine
    if fetch_result.is_err() {
        // Try alternative: fetch the branch directly
        let _ = Command::new("git")
            .current_dir(&git_root)
            .args([
                "fetch",
                "origin",
                &format!("pull/{}/head:{}", pr_number, branch),
            ])
            .status();
    }

    // Create worktree for the PR branch
    let branch_safe = git::sanitize_branch(&branch);
    let workspaces_dir = get_workspaces_dir()?;
    let repo_name = git_root
        .file_name()
        .context("Invalid git root")?
        .to_string_lossy();
    let wt_path = workspaces_dir.join(repo_name.as_ref()).join(&branch_safe);

    // Create repo subdirectory if needed
    let repo_dir = workspaces_dir.join(repo_name.as_ref());
    if !repo_dir.exists() {
        std::fs::create_dir_all(&repo_dir)?;
    }

    println!("{} Creating worktree...", "::".blue().bold());

    // Try to create worktree from the fetched branch
    let result = Command::new("git")
        .current_dir(&git_root)
        .args(["worktree", "add", wt_path.to_str().unwrap(), &branch])
        .output()?;

    if !result.status.success() {
        // Try with origin/branch
        let result = Command::new("git")
            .current_dir(&git_root)
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                &format!("origin/{}", branch),
            ])
            .output()?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            anyhow::bail!("Failed to create worktree: {}", stderr.trim());
        }
    }

    println!(
        "{} Worktree created at {}",
        "::".green().bold(),
        wt_path.display()
    );

    open(Some(wt_path.display().to_string()))
}

/// Garbage collect merged branches and their worktrees
pub fn gc(force: bool) -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let default_branch = git::get_default_branch(Some(&git_root));

    // Find merged branches
    let output = Command::new("git")
        .current_dir(&git_root)
        .args(["branch", "--merged", &format!("origin/{}", default_branch)])
        .output()
        .context("Failed to list merged branches")?;

    let merged_output = String::from_utf8_lossy(&output.stdout);
    let merged_branches: std::collections::HashSet<String> = merged_output
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches("* ")
                .trim_start_matches("+ ")
                .to_string()
        })
        .filter(|b| !b.is_empty() && b != &default_branch && !b.starts_with("remotes/"))
        .collect();

    // Find worktrees with merged branches (excluding main worktree)
    let to_delete: Vec<_> = worktrees
        .iter()
        .filter(|wt| {
            wt.path != git_root
                && !wt.branch.starts_with("detached:")
                && merged_branches.contains(&wt.branch)
        })
        .collect();

    if to_delete.is_empty() {
        println!("{} No merged worktrees to clean up!", "::".green().bold());

        // Still prune any dangling worktree refs
        let _ = Command::new("git")
            .current_dir(&git_root)
            .args(["worktree", "prune"])
            .status();

        return Ok(());
    }

    println!("{}", "Merged worktrees to delete:".bold());
    println!();
    for wt in &to_delete {
        println!("  {} {}", "✗".red(), wt.branch);
        println!("    {}", wt.path.display().to_string().dimmed());
    }
    println!();

    if !force {
        print!(
            "Delete {} worktree(s) and their branches? [y/N]: ",
            to_delete.len()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("{} Aborted", "::".yellow().bold());
            return Ok(());
        }
    }

    let mut deleted = 0;
    for wt in to_delete {
        // Kill tmux session if exists
        if let Ok(session) = get_session_name(&wt.path) {
            if tmux::session_exists(&session) {
                let _ = tmux::kill_session(&session);
            }
        }

        // Remove worktree
        if let Err(e) = git::remove_worktree(&git_root, &wt.path, true) {
            println!(
                "{} Failed to remove worktree {}: {}",
                "::".red().bold(),
                wt.branch,
                e
            );
            continue;
        }

        // Delete branch
        let _ = git::delete_branch(&git_root, &wt.branch, true);

        println!("{} Deleted {}", "::".green().bold(), wt.branch);
        deleted += 1;
    }

    // Prune worktree refs
    let _ = Command::new("git")
        .current_dir(&git_root)
        .args(["worktree", "prune"])
        .status();

    println!();
    println!(
        "{} Garbage collection complete: {} worktree(s) deleted",
        "::".green().bold(),
        deleted
    );

    Ok(())
}
