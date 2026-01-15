use crate::config::{AiTool, Config};
use crate::git;
use crate::onboarding;
use crate::tmux;
use anyhow::{Context, Result};
use colored::*;
use crossterm::{
    cursor::{MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear as TermClear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color as RatColor, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell as RatCell, Clear, List, ListItem, ListState, Row, Table as RatTable,
    },
    Frame, Terminal,
};
use std::io::{self, stdout, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Generate session name from directory
fn get_session_name(dir: &Path) -> Result<String> {
    let repo_name = dir
        .file_name()
        .context("Invalid directory")?
        .to_string_lossy();
    let branch = git::get_branch(dir)?;
    let branch_safe = git::sanitize_branch(&branch);
    Ok(format!("{}-{}", repo_name, branch_safe))
}

/// Generate window title for Ghostty tab: "repo/worktree [branch]"
fn get_window_title(dir: &Path) -> Result<String> {
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

/// Get the workspaces directory (~/.ws/workspaces/)
fn get_workspaces_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let ws_dir = home.join(".ws").join("workspaces");

    // Create directory if it doesn't exist
    if !ws_dir.exists() {
        std::fs::create_dir_all(&ws_dir).context("Failed to create workspaces directory")?;
    }

    Ok(ws_dir)
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
                    None => anyhow::bail!("Worktree not found: {}", t),
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
    let git_root = git::get_root(None).context("Not in a git repository")?;

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

/// Dependency information
struct Dependency {
    name: &'static str,
    brew_name: &'static str,
    description: &'static str,
    required: bool,
}

const DEPENDENCIES: &[Dependency] = &[
    Dependency {
        name: "tmux",
        brew_name: "tmux",
        description: "Terminal multiplexer for workspace layouts",
        required: true,
    },
    Dependency {
        name: "git",
        brew_name: "git",
        description: "Version control (for worktrees)",
        required: true,
    },
    Dependency {
        name: "fzf",
        brew_name: "fzf",
        description: "Fuzzy finder for interactive selection",
        required: true,
    },
    Dependency {
        name: "lazygit",
        brew_name: "lazygit",
        description: "Terminal UI for git",
        required: false,
    },
    Dependency {
        name: "droid",
        brew_name: "", // Not available via brew, it's Claude Code
        description: "Claude Code CLI (install from claude.ai)",
        required: false,
    },
];

/// Check and install dependencies
pub fn doctor(install: bool) -> Result<()> {
    println!("{}", "Workspace CLI Dependencies".bold());
    println!();

    let mut missing_required: Vec<&Dependency> = Vec::new();
    let mut missing_optional: Vec<&Dependency> = Vec::new();
    let mut all_ok = true;

    for dep in DEPENDENCIES {
        let found = which::which(dep.name).is_ok();
        let status = if found {
            "✓".green().to_string()
        } else {
            all_ok = false;
            if dep.required {
                missing_required.push(dep);
                "✗".red().to_string()
            } else {
                missing_optional.push(dep);
                "○".yellow().to_string()
            }
        };

        let req = if dep.required { "" } else { " (optional)" };
        println!("  {} {}{}", status, dep.name, req.dimmed());
        println!("    {}", dep.description.dimmed());
    }

    println!();

    if all_ok {
        println!("{} All dependencies installed!", "::".green().bold());
        return Ok(());
    }

    // Install missing dependencies
    if install {
        // Check for Homebrew
        if which::which("brew").is_err() {
            anyhow::bail!(
                "Homebrew is required to install dependencies. Install from https://brew.sh"
            );
        }

        let to_install: Vec<_> = missing_required
            .iter()
            .chain(missing_optional.iter())
            .filter(|d| !d.brew_name.is_empty())
            .collect();

        if to_install.is_empty() {
            println!("{} Nothing to install via Homebrew", "::".yellow().bold());
        } else {
            println!("{} Installing dependencies...", "::".blue().bold());
            println!();

            for dep in to_install {
                println!("  Installing {}...", dep.name);
                let result = Command::new("brew")
                    .args(["install", dep.brew_name])
                    .status();

                match result {
                    Ok(status) if status.success() => {
                        println!("    {}", "installed".green());
                    }
                    _ => {
                        println!("    {}", "failed".red());
                    }
                }
            }

            println!();
        }

        // Check for droid separately
        if missing_optional.iter().any(|d| d.name == "droid") {
            println!(
                "{} Note: 'droid' (Claude Code) must be installed manually:",
                "::".yellow().bold()
            );
            println!("  Visit https://claude.ai/download");
            println!();
        }

        println!(
            "{} Run 'ws doctor' to verify installation",
            "::".blue().bold()
        );
    } else if !missing_required.is_empty() {
        println!("{} Missing required dependencies!", "::".red().bold());
        println!("  Run {} to install", "ws doctor --install".cyan());
    } else {
        println!(
            "{} Some optional dependencies missing",
            "::".yellow().bold()
        );
        println!("  Run {} to install", "ws doctor --install".cyan());
    }

    Ok(())
}

/// Show status dashboard with worktrees and sessions using ratatui
pub fn status() -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let active_sessions = tmux::get_active_sessions();

    // Get repo name for session matching
    let repo_name = git_root
        .file_name()
        .context("Invalid git root")?
        .to_string_lossy()
        .to_string();

    // Build linked pairs and find orphans
    let mut all_entries: Vec<(String, String, String, bool, bool)> = Vec::new(); // (session, branch, path, is_main, has_session)
    let mut worktree_sessions: std::collections::HashSet<String> = std::collections::HashSet::new();

    for wt in &worktrees {
        if let Ok(session_name) = get_session_name(&wt.path) {
            let has_session = active_sessions.contains(&session_name);
            let is_main = wt.path == git_root;

            if has_session {
                worktree_sessions.insert(session_name.clone());
            }

            all_entries.push((
                session_name,
                wt.branch.clone(),
                wt.path.display().to_string(),
                is_main,
                has_session,
            ));
        }
    }

    // Find orphaned sessions (sessions without worktrees)
    let orphaned_sessions: Vec<String> = active_sessions
        .iter()
        .filter(|s| s.starts_with(&format!("{}-", repo_name)) && !worktree_sessions.contains(*s))
        .cloned()
        .collect();

    // Find orphaned worktrees (worktrees without sessions, excluding main)
    let orphaned_worktrees: Vec<String> = worktrees
        .iter()
        .filter(|wt| {
            if wt.path == git_root {
                return false;
            }
            if let Ok(name) = get_session_name(&wt.path) {
                !active_sessions.contains(&name)
            } else {
                false
            }
        })
        .map(|wt| wt.branch.clone())
        .collect();

    let has_orphans = !orphaned_sessions.is_empty() || !orphaned_worktrees.is_empty();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Draw the UI
    terminal.draw(|frame| {
        let area = frame.area();

        // Main vertical layout
        let chunks = if has_orphans {
            Layout::vertical([
                Constraint::Length(2), // Title
                Constraint::Min(5),    // Main table
                Constraint::Length(6), // Orphan tables
                Constraint::Length(2), // Tips
            ])
            .split(area)
        } else {
            Layout::vertical([
                Constraint::Length(2), // Title
                Constraint::Min(5),    // Main table
                Constraint::Length(2), // Success message
            ])
            .split(area)
        };

        // Title
        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", repo_name),
                Style::default()
                    .fg(RatColor::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Workspace Status"),
        ]);
        frame.render_widget(ratatui::widgets::Paragraph::new(title), chunks[0]);

        // Main table
        let header = Row::new(vec![
            RatCell::from("").style(Style::default()),
            RatCell::from("Session").style(Style::default().fg(RatColor::Cyan)),
            RatCell::from("Branch").style(Style::default().fg(RatColor::Cyan)),
            RatCell::from("Path").style(Style::default().fg(RatColor::Cyan)),
        ])
        .height(1);

        let rows: Vec<Row> = all_entries
            .iter()
            .map(|(session, branch, path, is_main, has_session)| {
                let status = if *has_session { "●" } else { "○" };
                let status_style = if *has_session {
                    Style::default().fg(RatColor::Green)
                } else {
                    Style::default().fg(RatColor::Yellow)
                };
                let main_marker = if *is_main { " (main)" } else { "" };
                let dim_style = Style::default().fg(RatColor::DarkGray);

                Row::new(vec![
                    RatCell::from(status).style(status_style),
                    RatCell::from(session.as_str()).style(if *has_session {
                        Style::default()
                    } else {
                        dim_style
                    }),
                    RatCell::from(format!("{}{}", branch, main_marker)),
                    RatCell::from(path.as_str()).style(dim_style),
                ])
            })
            .collect();

        let table = RatTable::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Percentage(30),
                Constraint::Percentage(25),
                Constraint::Percentage(45),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Worktrees & Sessions ")
                .border_style(Style::default().fg(RatColor::DarkGray)),
        );
        frame.render_widget(table, chunks[1]);

        if has_orphans {
            // Split bottom area for two orphan tables
            let orphan_chunks =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[2]);

            // Orphaned sessions table
            let session_rows: Vec<Row> = if orphaned_sessions.is_empty() {
                vec![Row::new(vec![
                    RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
                ])]
            } else {
                orphaned_sessions
                    .iter()
                    .map(|s| Row::new(vec![RatCell::from(s.as_str())]))
                    .collect()
            };
            let sessions_table = RatTable::new(session_rows, [Constraint::Percentage(100)]).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(
                        " Orphaned Sessions ",
                        Style::default().fg(RatColor::Red),
                    ))
                    .border_style(Style::default().fg(RatColor::DarkGray)),
            );
            frame.render_widget(sessions_table, orphan_chunks[0]);

            // Orphaned worktrees table
            let worktree_rows: Vec<Row> = if orphaned_worktrees.is_empty() {
                vec![Row::new(vec![
                    RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
                ])]
            } else {
                orphaned_worktrees
                    .iter()
                    .map(|b| Row::new(vec![RatCell::from(b.as_str())]))
                    .collect()
            };
            let worktrees_table = RatTable::new(worktree_rows, [Constraint::Percentage(100)])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(Span::styled(
                            " Orphaned Worktrees ",
                            Style::default().fg(RatColor::Yellow),
                        ))
                        .border_style(Style::default().fg(RatColor::DarkGray)),
                );
            frame.render_widget(worktrees_table, orphan_chunks[1]);

            // Tips
            let tips = ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::styled(" Tip: ", Style::default().fg(RatColor::DarkGray)),
                Span::styled("ws sync --create", Style::default().fg(RatColor::Cyan)),
                Span::raw(" to create sessions  "),
                Span::styled("ws sync --delete", Style::default().fg(RatColor::Cyan)),
                Span::raw(" to delete worktrees  "),
                Span::styled(
                    "q",
                    Style::default()
                        .fg(RatColor::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" to quit"),
            ]));
            frame.render_widget(tips, chunks[3]);
        } else {
            // Success message
            let success = ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::styled(" ✓ ", Style::default().fg(RatColor::Green)),
                Span::raw("All worktrees and sessions are in sync  "),
                Span::styled(
                    "q",
                    Style::default()
                        .fg(RatColor::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" to quit"),
            ]));
            frame.render_widget(success, chunks[2]);
        }
    })?;

    // Wait for 'q' to quit
    loop {
        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.code == crossterm::event::KeyCode::Char('q')
                    || key.code == crossterm::event::KeyCode::Esc
                {
                    break;
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

/// Configure workspace settings
pub fn config(key: Option<String>, value: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;

    match (key.as_deref(), value.as_deref()) {
        // Show all settings
        (None, None) => {
            println!("{}", "Workspace Configuration".bold());
            println!();
            println!("  {} = {}", "ai_tool".cyan(), cfg.ai_tool);

            let installed = if cfg.is_ai_tool_installed() {
                "installed".green()
            } else {
                "not installed".red()
            };
            println!("           ({})", installed);

            println!();
            println!("{}", "Available AI tools:".dimmed());
            for tool in AiTool::all() {
                let marker = if *tool == cfg.ai_tool { "●" } else { " " };
                let installed = if which::which(tool.binary()).is_ok() {
                    "✓".green()
                } else {
                    "✗".red()
                };
                println!(
                    "  {} {} {} - {}",
                    marker,
                    installed,
                    tool.command(),
                    tool.name()
                );
            }

            println!();
            println!("Set with: {} <key> <value>", "ws config".cyan());
            println!("Example:  {} ai_tool claude", "ws config".cyan());
        }

        // Show specific setting
        (Some(k), None) => match k {
            "ai_tool" => {
                println!("{}", cfg.ai_tool);
            }
            _ => {
                anyhow::bail!("Unknown setting: {}", k);
            }
        },

        // Set a value
        (Some(k), Some(v)) => match k {
            "ai_tool" => {
                let tool = AiTool::from_str(v).context(format!(
                    "Unknown AI tool: {}. Valid options: droid, claude, codex, gemini, copilot",
                    v
                ))?;

                cfg.ai_tool = tool;
                cfg.save()?;

                println!("{} Set ai_tool to {}", "::".green().bold(), tool.name());

                if !cfg.is_ai_tool_installed() {
                    println!(
                        "{} Warning: {} is not installed",
                        "::".yellow().bold(),
                        tool.binary()
                    );
                }
            }
            _ => {
                anyhow::bail!("Unknown setting: {}", k);
            }
        },

        (None, Some(_)) => {
            anyhow::bail!("Please specify a setting name");
        }
    }

    Ok(())
}

/// Re-run setup wizard, backing up existing config
pub fn init() -> Result<()> {
    let config_path = Config::path()?;

    // Backup existing config if it exists
    if config_path.exists() {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let backup_path = config_path.with_extension(format!("toml.{}.bak", timestamp));

        std::fs::rename(&config_path, &backup_path).context("Failed to backup config")?;

        println!(
            "{} Backed up config to {}",
            "::".blue().bold(),
            backup_path.display()
        );
    }

    // Run onboarding
    if let Some(result) = onboarding::run_onboarding()? {
        // Save names before moving
        let ai_name = result.ai_tool.name().to_string();
        let git_name = result.git_tool.name().to_string();
        let explorer_name = result.explorer_tool.name().to_string();

        let config = Config {
            ai_tool: result.ai_tool,
            git_tool: result.git_tool,
            explorer_tool: result.explorer_tool,
        };
        config.save()?;

        println!();
        println!(
            "{} Configuration saved! AI: {}, Git: {}, Explorer: {}",
            "::".green().bold(),
            ai_name,
            git_name,
            explorer_name
        );
        println!();

        // If a path was selected, open it
        if let Some(path) = result.path {
            return open(Some(path.to_string_lossy().to_string()));
        }
    }

    Ok(())
}

/// Switch AI tool in current tmux session
pub fn ai(tool_name: Option<String>) -> Result<()> {
    // Get current config
    let mut cfg = Config::load()?;

    // Determine which tool to use
    let tool = if let Some(name) = tool_name {
        AiTool::from_str(&name).context(format!(
            "Unknown AI tool: {}. Valid options: droid, claude, codex, gemini, copilot",
            name
        ))?
    } else {
        // Show TUI selector
        match run_ai_selector(cfg.ai_tool)? {
            Some(t) => t,
            None => return Ok(()), // User cancelled
        }
    };

    // Update config
    cfg.ai_tool = tool;
    cfg.save()?;

    // Check if we're in a tmux session
    if std::env::var("TMUX").is_err() {
        println!("{} AI tool set to {}", "::".green().bold(), tool.name());
        println!(
            "{} Not in tmux session, config updated for next session",
            "::".yellow().bold()
        );
        return Ok(());
    }

    // Get current session name (works even in popup)
    let session_output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .context("Failed to get tmux session")?;

    let session = String::from_utf8_lossy(&session_output.stdout)
        .trim()
        .to_string();

    // If we're in a popup, get the client's session instead
    let session = if session.starts_with("popup") {
        let client_output = Command::new("tmux")
            .args(["display-message", "-p", "-t", "{last}", "#{session_name}"])
            .output()
            .context("Failed to get client session")?;
        let client_session = String::from_utf8_lossy(&client_output.stdout)
            .trim()
            .to_string();
        if client_session.is_empty() || client_session.starts_with("popup") {
            session
        } else {
            client_session
        }
    } else {
        session
    };

    if session.is_empty() {
        anyhow::bail!("Could not determine current tmux session");
    }

    // The AI pane is always pane 2 in our layout (both large and small)
    let target = format!("{}:0.2", session);

    // Check if target pane exists
    let check_pane = Command::new("tmux")
        .args(["has-session", "-t", &target])
        .output();

    if check_pane.map(|o| !o.status.success()).unwrap_or(true) {
        println!("{} AI tool set to {}", "::".green().bold(), tool.name());
        println!(
            "{} Pane {} not found, config updated for next session",
            "::".yellow().bold(),
            target
        );
        return Ok(());
    }

    // Kill any running process in the pane properly
    kill_pane_processes(&target)?;

    // Small delay to let the process exit and shell reset
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Clear terminal and command line, then start new tool
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-u", "clear", "Enter"])
        .output()
        .context("Failed to clear terminal")?;

    // Small delay for clear to complete
    std::thread::sleep(std::time::Duration::from_millis(50));

    Command::new("tmux")
        .args(["send-keys", "-t", &target, tool.command(), "Enter"])
        .output()
        .context("Failed to send new command")?;

    println!(
        "{} Switched to {} in pane {}",
        "::".green().bold(),
        tool.name(),
        target
    );

    Ok(())
}

/// Kill any running process in a tmux pane
fn kill_pane_processes(target: &str) -> Result<()> {
    // Get the pane's shell PID
    let pane_pid_output = Command::new("tmux")
        .args(["display-message", "-p", "-t", target, "#{pane_pid}"])
        .output()
        .context("Failed to get pane PID")?;

    let pane_pid = String::from_utf8_lossy(&pane_pid_output.stdout)
        .trim()
        .to_string();

    if pane_pid.is_empty() {
        return Ok(());
    }

    // Find child processes of the shell
    let children_output = Command::new("pgrep").args(["-P", &pane_pid]).output();

    if let Ok(output) = children_output {
        let children = String::from_utf8_lossy(&output.stdout);
        for child_pid in children.lines() {
            let child_pid = child_pid.trim();
            if !child_pid.is_empty() {
                // Send SIGTERM first (graceful shutdown)
                let _ = Command::new("kill").args(["-TERM", child_pid]).output();
            }
        }
    }

    // Also send Ctrl+C as fallback (in case process ignores SIGTERM briefly)
    let _ = Command::new("tmux")
        .args(["send-keys", "-t", target, "C-c"])
        .output();

    Ok(())
}

/// TUI selector for AI tools
fn run_ai_selector(current: AiTool) -> Result<Option<AiTool>> {
    let tools = AiTool::all();
    let current_idx = tools.iter().position(|t| *t == current).unwrap_or(0);

    let mut list_state = ListState::default();
    list_state.select(Some(current_idx));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected: Option<AiTool> = None;

    loop {
        terminal.draw(|frame| {
            draw_ai_selector(frame, tools, &mut list_state, current);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = list_state.selected().unwrap_or(0);
                            let new_i = if i == 0 { tools.len() - 1 } else { i - 1 };
                            list_state.select(Some(new_i));
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = list_state.selected().unwrap_or(0);
                            let new_i = if i >= tools.len() - 1 { 0 } else { i + 1 };
                            list_state.select(Some(new_i));
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            if let Some(i) = list_state.selected() {
                                selected = Some(tools[i]);
                            }
                            break;
                        }
                        KeyCode::Esc | KeyCode::Char('q') => {
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Restore terminal with full cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        TermClear(ClearType::All),
        MoveTo(0, 0),
        Show
    )?;
    // Flush to ensure all escape sequences are written before popup closes
    terminal.backend_mut().flush()?;

    Ok(selected)
}

fn draw_ai_selector(
    frame: &mut Frame,
    tools: &[AiTool],
    list_state: &mut ListState,
    current: AiTool,
) {
    let area = frame.area();

    // Calculate centered popup area
    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_height = (tools.len() as u16 + 6).min(area.height.saturating_sub(4));
    let popup_x = (area.width - popup_width) / 2;
    let popup_y = (area.height - popup_height) / 2;

    let popup_area = ratatui::layout::Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear background
    frame.render_widget(Clear, popup_area);

    // Build list items
    let items: Vec<ListItem> = tools
        .iter()
        .map(|tool| {
            let installed = which::which(tool.binary()).is_ok();
            let is_current = *tool == current;

            let marker = if is_current { "*" } else { " " };
            let status = if installed {
                Span::styled(" [installed]", Style::default().fg(RatColor::Green))
            } else {
                Span::styled(" [not found]", Style::default().fg(RatColor::Red))
            };

            ListItem::new(Line::from(vec![
                Span::raw(format!("{} ", marker)),
                Span::styled(tool.name(), Style::default().fg(RatColor::White)),
                status,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RatColor::Cyan))
                .title(" Switch AI Tool ")
                .style(Style::default().bg(RatColor::Black)),
        )
        .highlight_style(
            Style::default()
                .bg(RatColor::DarkGray)
                .fg(RatColor::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    // Split popup for list and footer
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(popup_area);

    frame.render_stateful_widget(list, chunks[0], list_state);

    // Footer
    let footer = ratatui::widgets::Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(RatColor::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(RatColor::Cyan)),
        Span::raw(" select  "),
        Span::styled("q", Style::default().fg(RatColor::Cyan)),
        Span::raw(" cancel"),
    ]))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().bg(RatColor::Black));
    frame.render_widget(footer, chunks[1]);
}

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
        .map(|l| l.trim().trim_start_matches("* ").to_string())
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

/// Update ws and texplore via Homebrew
pub fn update() -> Result<()> {
    // Check if brew is installed
    if which::which("brew").is_err() {
        anyhow::bail!("Homebrew is required. Install from https://brew.sh");
    }

    println!("{} Checking for updates...", "::".blue().bold());

    // Get current version
    let current_version = env!("CARGO_PKG_VERSION");
    println!(
        "{} Current version: {}",
        "::".blue().bold(),
        current_version
    );

    // Update brew
    println!("{} Updating Homebrew...", "::".blue().bold());
    let _ = Command::new("brew").args(["update"]).status();

    // Upgrade ws and texplore
    println!("{} Upgrading ws and texplore...", "::".blue().bold());
    let status = Command::new("brew")
        .args(["upgrade", "ws", "texplore"])
        .status()
        .context("Failed to run brew upgrade")?;

    if status.success() {
        // Check new version
        let output = Command::new("ws").args(["--version"]).output();
        if let Ok(out) = output {
            let new_version = String::from_utf8_lossy(&out.stdout);
            println!("{} Updated to: {}", "::".green().bold(), new_version.trim());
        } else {
            println!("{} Update complete!", "::".green().bold());
        }
    } else {
        println!(
            "{} Already up to date or no updates available",
            "::".yellow().bold()
        );
    }

    Ok(())
}
