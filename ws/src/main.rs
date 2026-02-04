mod commands;
mod config;
mod git;
mod onboarding;
mod tmux;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::StatusAction;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ws")]
#[command(about = "Workspace CLI for git worktrees with tmux layouts")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Output status bar info for tmux (internal use)
    #[arg(long, hide = true)]
    status_bar: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open workspace for a directory, branch, or worktree name
    #[command(alias = "o")]
    Open {
        /// Path, branch name, or worktree directory name
        target: Option<String>,
    },

    /// Create new worktree and open workspace
    #[command(alias = "n")]
    New {
        /// Branch name for the new worktree
        branch: String,

        /// Base branch to create from (auto-detects: main, master, or develop)
        #[arg(short, long)]
        from: Option<String>,
    },

    /// Interactive worktree selector (fzf)
    #[command(alias = "s")]
    Select {
        /// Directly open this path (used by lazygit integration)
        #[arg(long)]
        path: Option<PathBuf>,
    },

    /// Delete worktree and its tmux session
    #[command(alias = "d", alias = "rm")]
    Delete {
        /// Branch name or path of the worktree to delete
        target: String,

        /// Force delete even with uncommitted changes
        #[arg(short, long)]
        force: bool,
    },

    /// Reload tmux session for a worktree (recreates with current config)
    #[command(alias = "r")]
    Reload {
        /// Branch name, path, or worktree directory name (defaults to current)
        target: Option<String>,
    },

    /// Sync tmux sessions with worktrees (clean up orphans)
    Sync {
        /// Create sessions for worktrees that don't have one
        #[arg(long)]
        create: bool,

        /// Delete worktrees (and branches) that don't have active sessions
        #[arg(long)]
        delete: bool,
    },

    /// Check and install dependencies
    Doctor {
        /// Install missing dependencies with Homebrew
        #[arg(long)]
        install: bool,
    },

    /// Show status dashboard with worktrees and sessions
    Status,

    /// Configure workspace settings
    Config {
        /// Setting to configure (e.g., ai_tool)
        key: Option<String>,

        /// Value to set
        value: Option<String>,
    },

    /// Re-run setup wizard (backs up existing config)
    Init,

    /// Switch AI tool in current session
    #[command(alias = "a")]
    Ai {
        /// AI tool to switch to (shows selector if not provided)
        tool: Option<String>,
    },

    /// Clone a repository and set up workspace structure
    #[command(alias = "c")]
    Clone {
        /// Repository URL to clone
        url: String,
    },

    /// Create a pull request from current worktree
    Pr {
        #[command(subcommand)]
        action: Option<PrCommands>,
    },

    /// Review a pull request in a new worktree
    Review {
        /// PR number to review
        number: u32,
    },

    /// Garbage collect merged branches and their worktrees
    Gc {
        /// Force delete without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Update ws and texplore via Homebrew
    Update,

    /// Toggle tmux layout based on display size
    #[command(alias = "l")]
    Layout {
        /// Force expand to 5-pane layout
        #[arg(long)]
        expand: bool,
        /// Force shrink to 3-pane layout
        #[arg(long)]
        shrink: bool,
    },
}

#[derive(Subcommand)]
enum PrCommands {
    /// List PRs for branches with worktrees
    List,
}

fn handle_status_action(action: StatusAction) -> Result<()> {
    match action {
        StatusAction::None => Ok(()),
        StatusAction::Open(path) => commands::open(Some(path.to_string_lossy().to_string())),
        StatusAction::Ai => commands::ai(None),
        StatusAction::ReviewPr(number) => commands::review(number),
    }
}

/// Print status bar info for tmux (called via #(ws --status-bar "dir"))
/// Uses caching to avoid calling gh on every tmux refresh
fn print_status_bar(dir: &str) {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    let dir_path = Path::new(dir);

    // Get branch name (fast, no caching needed)
    let branch = git::get_branch(dir_path).unwrap_or_default();
    if branch.is_empty() {
        return;
    }

    // Cache file based on directory hash
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("ws-status");
    let _ = fs::create_dir_all(&cache_dir);

    let dir_hash = dir
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let cache_file = cache_dir.join(format!("{}.cache", dir_hash));

    // Check cache (60 second TTL)
    let cache_ttl = Duration::from_secs(60);
    let cached_pr_info = if let Ok(metadata) = fs::metadata(&cache_file) {
        if let Ok(modified) = metadata.modified() {
            if SystemTime::now()
                .duration_since(modified)
                .unwrap_or(cache_ttl)
                < cache_ttl
            {
                fs::read_to_string(&cache_file).ok()
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let pr_info = cached_pr_info.unwrap_or_else(|| {
        let info = fetch_pr_info_for_branch(dir_path, &branch);
        let _ = fs::write(&cache_file, &info);
        info
    });

    // Format: "branch | #123 Title ✓" or just "branch" if no PR
    if pr_info.is_empty() {
        print!("{}", branch);
    } else {
        print!("{} │ {}", branch, pr_info);
    }
}

/// Fetch PR info for a branch (number, title, check status)
fn fetch_pr_info_for_branch(dir: &std::path::Path, branch: &str) -> String {
    // Skip if gh is not installed
    if which::which("gh").is_err() {
        return String::new();
    }

    // Query gh for PR on this branch
    let output = std::process::Command::new("gh")
        .current_dir(dir)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--json",
            "number,title,statusCheckRollup",
            "--limit",
            "1",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return String::new(),
    };

    let prs: Vec<serde_json::Value> = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };

    let pr = match prs.first() {
        Some(p) => p,
        None => return String::new(),
    };

    let number = pr["number"].as_u64().unwrap_or(0);
    let title = pr["title"].as_str().unwrap_or("");

    // Parse check status
    let check_icon = parse_check_status_icon(&pr["statusCheckRollup"]);

    // Truncate title if too long
    let title_short: String = title.chars().take(30).collect();
    let title_display = if title.len() > 30 {
        format!("{}...", title_short)
    } else {
        title_short
    };

    format!("#{} {} {}", number, title_display, check_icon)
}

fn parse_check_status_icon(rollup: &serde_json::Value) -> &'static str {
    let checks = match rollup.as_array() {
        Some(arr) => arr,
        None => return "",
    };

    if checks.is_empty() {
        return "";
    }

    let mut has_pending = false;
    let mut has_failure = false;

    for check in checks {
        // StatusContext uses "state", CheckRun uses "conclusion" and "status"
        let state = check["state"].as_str().unwrap_or("");
        let conclusion = check["conclusion"].as_str().unwrap_or("");
        let status = check["status"].as_str().unwrap_or("");

        // Check for failures
        if state == "FAILURE"
            || state == "ERROR"
            || conclusion == "FAILURE"
            || conclusion == "failure"
            || conclusion == "ERROR"
            || conclusion == "error"
        {
            has_failure = true;
        }
        // Check for pending (but not if already marked as success via state)
        else if state == "PENDING"
            || state == "EXPECTED"
            || status == "IN_PROGRESS"
            || status == "QUEUED"
            || status == "PENDING"
        {
            has_pending = true;
        }
        // If it's a CheckRun (has status field) with no conclusion yet, it's pending
        else if !status.is_empty() && conclusion.is_empty() && status != "COMPLETED" {
            has_pending = true;
        }
    }

    if has_failure {
        "✗"
    } else if has_pending {
        "○"
    } else {
        "✓"
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --status-bar flag (for tmux status bar, needs to be fast)
    if let Some(dir) = cli.status_bar {
        print_status_bar(&dir);
        return Ok(());
    }

    // Check for first-time setup (skip for config and init commands)
    let onboarding_path = if !matches!(
        cli.command,
        Some(Commands::Config { .. }) | Some(Commands::Init)
    ) {
        onboarding::check_and_run_onboarding()?
    } else {
        None
    };

    // If onboarding selected a path, use it
    if let Some(path) = onboarding_path {
        return commands::open(Some(path.to_string_lossy().to_string()));
    }

    match cli.command {
        Some(Commands::Open { target }) => commands::open(target),
        Some(Commands::New { branch, from }) => {
            let base = from.unwrap_or_else(|| git::get_default_branch(None));
            commands::new(&branch, &base)
        }

        Some(Commands::Select { path }) => commands::select(path),
        Some(Commands::Delete { target, force }) => commands::delete(&target, force),
        Some(Commands::Reload { target }) => commands::reload(target),
        Some(Commands::Sync { create, delete }) => commands::sync(create, delete),
        Some(Commands::Doctor { install }) => commands::doctor(install),
        Some(Commands::Status) => handle_status_action(commands::status()?),
        Some(Commands::Config { key, value }) => commands::config(key, value),
        Some(Commands::Init) => commands::init(),
        Some(Commands::Ai { tool }) => commands::ai(tool),

        Some(Commands::Clone { url }) => commands::clone_repo(&url),
        Some(Commands::Pr { action }) => match action {
            Some(PrCommands::List) => commands::pr_list(),
            None => commands::pr_create(),
        },
        Some(Commands::Review { number }) => commands::review(number),
        Some(Commands::Gc { force }) => commands::gc(force),
        Some(Commands::Update) => commands::update(),
        Some(Commands::Layout { expand, shrink }) => commands::layout(expand, shrink),
        None => {
            // Check if config exists AND we're in a git repo - if so, show dashboard
            let config_path = crate::config::Config::path()?;
            let in_git_repo = git::get_root(None).is_ok();

            if config_path.exists() && in_git_repo {
                // Config exists and in git repo - show dashboard (plasma + status)
                handle_status_action(commands::dashboard()?)
            } else {
                // No config or not in git repo - run onboarding
                if let Some(result) = onboarding::run_onboarding()? {
                    let config = crate::config::Config {
                        ai_tool: result.ai_tool,
                        git_tool: result.git_tool,
                        explorer_tool: result.explorer_tool,
                    };
                    config.save()?;
                    if let Some(path) = result.path {
                        commands::open(Some(path.to_string_lossy().to_string()))
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                }
            }
        }
    }
}
