use super::{get_session_name, get_window_title, get_workspaces_dir};
use crate::config::Config;
use crate::git;
use crate::tmux;
use anyhow::{Context, Result};
use colored::*;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Warn if configured panel tools are not installed
fn warn_missing_tools() -> Result<()> {
    let cfg = Config::load()?;
    let mut warnings = Vec::new();

    if !cfg.is_ai_tool_installed() {
        warnings.push(format!("AI tool '{}' not found", cfg.ai_tool.command()));
    }
    if !cfg.is_git_tool_installed() {
        warnings.push(format!("Git tool '{}' not found", cfg.git_tool.command()));
    }
    if !cfg.is_explorer_tool_installed() {
        warnings.push(format!(
            "Explorer '{}' not found",
            cfg.explorer_tool.command()
        ));
    }

    for warning in warnings {
        println!("{} {}", "⚠".yellow().bold(), warning.yellow());
    }

    Ok(())
}

/// Open workspace for a directory, branch name, or worktree name
pub fn open(target: Option<String>) -> Result<()> {
    let dir = match target {
        Some(t) => {
            let path = PathBuf::from(&t);
            // If it's an existing path, use it directly
            if path.exists() {
                path
            } else {
                // Try to find it as a worktree by branch/name
                let git_root = git::get_root(None).context("Not in a git repository")?;
                match git::find_worktree(&git_root, &t)? {
                    Some(wt) => wt.path,
                    None => {
                        // Ask user if they want to create the worktree
                        let default_branch = git::get_default_branch(Some(&git_root));
                        println!("{} Worktree '{}' not found.", "::".yellow().bold(), t);
                        print!("Create new worktree from {}? [Y/n]: ", default_branch);
                        io::stdout().flush()?;

                        let mut input = String::new();
                        io::stdin().lock().read_line(&mut input)?;
                        let input = input.trim().to_lowercase();

                        if input.is_empty() || input == "y" || input == "yes" {
                            // Create the worktree and return its path
                            return new(&t, &default_branch);
                        } else {
                            anyhow::bail!("Aborted");
                        }
                    }
                }
            }
        }
        None => git::get_root(None).unwrap_or_else(|_| std::env::current_dir().unwrap()),
    };

    let session = get_session_name(&dir)?;

    if tmux::session_exists(&session) {
        println!(
            "{} Attaching to existing session: {}",
            "::".blue().bold(),
            session
        );
        tmux::attach(&session)?;
        return Ok(());
    }

    // Warn about missing tools
    warn_missing_tools()?;

    let window_title = get_window_title(&dir)?;
    println!("{} Creating workspace: {}", "::".blue().bold(), session);
    tmux::create_session_with_title(&session, &dir, &window_title)?;
    tmux::attach(&session)?;

    Ok(())
}

/// Create new worktree and open workspace
pub fn new(branch: &str, base: &str) -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;

    let repo_name = git_root
        .file_name()
        .context("Invalid git root")?
        .to_string_lossy()
        .to_string();

    let branch_safe = git::sanitize_branch(branch);
    let workspaces_dir = get_workspaces_dir()?;
    let wt_path = workspaces_dir.join(&repo_name).join(&branch_safe);

    // Create repo subdirectory if needed
    let repo_dir = workspaces_dir.join(&repo_name);
    if !repo_dir.exists() {
        std::fs::create_dir_all(&repo_dir).context("Failed to create repo directory")?;
    }

    if wt_path.exists() {
        println!(
            "{} Worktree already exists at {}",
            "::".yellow().bold(),
            wt_path.display()
        );
        println!("{} Opening existing worktree...", "::".blue().bold());
        return open(Some(wt_path.display().to_string()));
    }

    println!(
        "{} Creating worktree '{}' from '{}'...",
        "::".blue().bold(),
        branch,
        base
    );

    git::create_worktree(&git_root, branch, base, &wt_path)?;

    println!(
        "{} Worktree created at {}",
        "::".green().bold(),
        wt_path.display()
    );

    open(Some(wt_path.display().to_string()))
}

/// List all worktrees with session status
pub fn list() -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let active_sessions = tmux::get_active_sessions();

    println!("{}", "Worktrees".bold());
    println!();

    for wt in worktrees {
        let session_name = get_session_name(&wt.path)?;
        let status = if active_sessions.contains(&session_name) {
            "●".green().to_string()
        } else {
            " ".to_string()
        };

        let main_marker = if wt.path == git_root {
            " (main)".yellow().to_string()
        } else {
            String::new()
        };

        println!("  {} {}{}", status, wt.branch, main_marker);
        println!("    {}", wt.path.display().to_string().blue());
        println!();
    }

    Ok(())
}

/// Interactive worktree selector with fzf
pub fn select(direct_path: Option<PathBuf>) -> Result<()> {
    // If direct path provided, just open it
    if let Some(path) = direct_path {
        return open(Some(path.display().to_string()));
    }

    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let active_sessions = tmux::get_active_sessions();

    // Check if fzf is available
    if which::which("fzf").is_err() {
        anyhow::bail!(
            "fzf is required for interactive selection. Install it with: brew install fzf"
        );
    }

    // Build options for fzf
    let mut options: Vec<String> = worktrees
        .iter()
        .map(|wt| {
            let session_name = get_session_name(&wt.path).unwrap_or_default();
            let status = if active_sessions.contains(&session_name) {
                "●"
            } else {
                " "
            };
            format!("{} {}|{}", status, wt.branch, wt.path.display())
        })
        .collect();

    options.push("+ Create new worktree...|__CREATE__".to_string());

    // Run fzf
    let mut fzf = Command::new("fzf")
        .args([
            "--ansi",
            "--no-sort",
            "--header=Select worktree (● = active session)",
            "--delimiter=|",
            "--with-nth=1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to start fzf")?;

    {
        let stdin = fzf.stdin.as_mut().context("Failed to get fzf stdin")?;
        for opt in &options {
            writeln!(stdin, "{}", opt)?;
        }
    }

    let output = fzf.wait_with_output()?;

    if !output.status.success() {
        // User cancelled
        return Ok(());
    }

    let selected = String::from_utf8_lossy(&output.stdout);
    let selected = selected.trim();

    if selected.is_empty() {
        return Ok(());
    }

    // Extract path from selection
    let path = selected.split('|').nth(1).context("Invalid selection")?;

    if path == "__CREATE__" {
        return interactive_create();
    }

    open(Some(path.to_string()))
}

fn interactive_create() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    let git_root = git::get_root(None).ok();
    let default_branch = git::get_default_branch(git_root.as_deref());

    print!("Branch name: ");
    stdout.flush()?;

    let mut branch = String::new();
    stdin.lock().read_line(&mut branch)?;
    let branch = branch.trim();

    if branch.is_empty() {
        anyhow::bail!("No branch name provided");
    }

    print!("Base branch [{}]: ", default_branch);
    stdout.flush()?;

    let mut base = String::new();
    stdin.lock().read_line(&mut base)?;
    let base = base.trim();
    let base = if base.is_empty() {
        &default_branch
    } else {
        base
    };

    new(branch, base)
}

/// Reload tmux session for a worktree (kill and recreate with current config)
pub fn reload(target: Option<String>) -> Result<()> {
    let dir = match target {
        Some(t) => {
            let path = PathBuf::from(&t);
            if path.exists() {
                path
            } else {
                let git_root = git::get_root(None).context("Not in a git repository")?;
                match git::find_worktree(&git_root, &t)? {
                    Some(wt) => wt.path,
                    None => {
                        // Worktree not found, ask if user wants to create it
                        print!("Worktree '{}' not found. Create it? [y/N]: ", t);
                        io::stdout().flush()?;
                        let mut input = String::new();
                        io::stdin().lock().read_line(&mut input)?;
                        if input.trim().eq_ignore_ascii_case("y") {
                            let base = git::get_default_branch(Some(&git_root));
                            return new(&t, &base);
                        } else {
                            anyhow::bail!("Worktree not found: {}", t);
                        }
                    }
                }
            }
        }
        None => git::get_root(None).unwrap_or_else(|_| std::env::current_dir().unwrap()),
    };

    let session = get_session_name(&dir)?;

    // Kill existing session if it exists
    if tmux::session_exists(&session) {
        println!("{} Killing session: {}", "::".blue().bold(), session);
        tmux::kill_session(&session)?;
    }

    // Warn about missing tools
    warn_missing_tools()?;

    // Recreate the session with current config
    let window_title = get_window_title(&dir)?;
    println!("{} Recreating workspace: {}", "::".blue().bold(), session);
    tmux::create_session_with_title(&session, &dir, &window_title)?;
    tmux::attach(&session)?;

    Ok(())
}

/// Delete worktree, tmux session, and local branch
pub fn delete(target: &str, force: bool) -> Result<()> {
    let target_path = Path::new(target);

    // Get the main worktree root (original repo), not the linked worktree's root
    let git_root = if target_path.is_absolute() || target_path.exists() {
        git::get_main_worktree_root(Some(target_path)).context("Not in a git repository")?
    } else {
        git::get_main_worktree_root(None).context("Not in a git repository")?
    };

    // Find the worktree
    let worktree = git::find_worktree(&git_root, target)?
        .context(format!("Worktree not found: {}", target))?;

    // Check if it's the main worktree
    if worktree.path == git_root {
        anyhow::bail!("Cannot delete the main worktree");
    }

    // Check if it's a detached worktree (no branch to delete)
    let is_detached = worktree.branch.starts_with("detached:");
    let branch_name = worktree.branch.clone();

    let session_name = get_session_name(&worktree.path)?;

    // Kill tmux session if it exists
    if tmux::session_exists(&session_name) {
        println!("{} Killing session: {}", "::".blue().bold(), session_name);
        tmux::kill_session(&session_name)?;
    }

    // Remove the worktree
    println!(
        "{} Removing worktree: {}",
        "::".blue().bold(),
        worktree.path.display()
    );
    git::remove_worktree(&git_root, &worktree.path, force)?;

    // Delete the local branch (unless detached)
    if !is_detached {
        println!("{} Deleting branch: {}", "::".blue().bold(), branch_name);
        git::delete_branch(&git_root, &branch_name, force)?;
    }

    println!(
        "{} Deleted worktree, session, and branch for '{}'",
        "::".green().bold(),
        branch_name
    );

    Ok(())
}

/// Sync tmux sessions with worktrees
pub fn sync(create_missing: bool, delete_unused: bool) -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let active_sessions = tmux::get_active_sessions();

    // Get repo name for session matching
    let repo_name = git_root
        .file_name()
        .context("Invalid git root")?
        .to_string_lossy();

    // Build set of valid session names
    let valid_sessions: std::collections::HashSet<String> = worktrees
        .iter()
        .filter_map(|wt| get_session_name(&wt.path).ok())
        .collect();

    // Find orphaned sessions (sessions without worktrees)
    let orphaned: Vec<&String> = active_sessions
        .iter()
        .filter(|s| s.starts_with(&format!("{}-", repo_name)) && !valid_sessions.contains(*s))
        .collect();

    // Find worktrees without sessions (excluding main worktree)
    let unused: Vec<_> = worktrees
        .iter()
        .filter(|wt| {
            // Skip main worktree
            if wt.path == git_root {
                return false;
            }
            if let Ok(name) = get_session_name(&wt.path) {
                !active_sessions.contains(&name)
            } else {
                false
            }
        })
        .collect();

    if orphaned.is_empty() && unused.is_empty() {
        println!("{} Everything is in sync!", "::".green().bold());
        return Ok(());
    }

    let mut killed = 0;
    let mut created = 0;
    let mut deleted = 0;

    // Report and clean up orphaned sessions
    if !orphaned.is_empty() {
        println!("{}", "Orphaned sessions (no worktree):".bold());
        for session in &orphaned {
            println!("  {} {}", "✗".red(), session);
            tmux::kill_session(session)?;
            println!("    {}", "killed".dimmed());
            killed += 1;
        }
        println!();
    }

    // Report/handle worktrees without sessions
    if !unused.is_empty() {
        println!("{}", "Worktrees without sessions:".bold());
        for wt in &unused {
            let session_name = get_session_name(&wt.path)?;
            let is_detached = wt.branch.starts_with("detached:");

            if delete_unused {
                println!("  {} {}", "✗".red(), wt.branch);

                // Remove worktree
                git::remove_worktree(&git_root, &wt.path, false)?;
                println!("    {}", "worktree removed".dimmed());

                // Delete branch if not detached
                if !is_detached {
                    git::delete_branch(&git_root, &wt.branch, false)?;
                    println!("    {}", "branch deleted".dimmed());
                }
                deleted += 1;
            } else if create_missing {
                println!(
                    "  {} {} ({})",
                    "○".yellow(),
                    wt.branch,
                    session_name.dimmed()
                );
                let window_title = get_window_title(&wt.path)?;
                tmux::create_session_with_title(&session_name, &wt.path, &window_title)?;
                println!("    {}", "created".green());
                created += 1;
            } else {
                println!(
                    "  {} {} ({})",
                    "○".yellow(),
                    wt.branch,
                    session_name.dimmed()
                );
            }
        }

        if !delete_unused && !create_missing {
            println!();
            println!("  {} to create sessions", "ws sync --create".cyan());
            println!("  {} to delete worktrees", "ws sync --delete".cyan());
        }
        println!();
    }

    println!(
        "{} Sync complete: {} sessions killed, {} sessions created, {} worktrees deleted",
        "::".green().bold(),
        killed,
        created,
        deleted
    );

    Ok(())
}
