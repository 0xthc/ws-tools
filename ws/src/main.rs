mod commands;
mod config;
mod git;
mod onboarding;
mod tmux;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ws")]
#[command(about = "Workspace CLI for git worktrees with tmux layouts")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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

    /// List all worktrees with session status
    #[command(alias = "l", alias = "ls")]
    List,

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

    /// Quick switch to a worktree by branch name
    #[command(alias = "sw")]
    Switch {
        /// Branch name to switch to
        branch: String,
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
}

#[derive(Subcommand)]
enum PrCommands {
    /// List PRs for branches with worktrees
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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
        Some(Commands::List) => commands::list(),
        Some(Commands::Select { path }) => commands::select(path),
        Some(Commands::Delete { target, force }) => commands::delete(&target, force),
        Some(Commands::Reload { target }) => commands::reload(target),
        Some(Commands::Sync { create, delete }) => commands::sync(create, delete),
        Some(Commands::Doctor { install }) => commands::doctor(install),
        Some(Commands::Status) => commands::status(),
        Some(Commands::Config { key, value }) => commands::config(key, value),
        Some(Commands::Init) => commands::init(),
        Some(Commands::Ai { tool }) => commands::ai(tool),
        Some(Commands::Switch { branch }) => commands::switch(&branch),
        Some(Commands::Clone { url }) => commands::clone_repo(&url),
        Some(Commands::Pr { action }) => match action {
            Some(PrCommands::List) => commands::pr_list(),
            None => commands::pr_create(),
        },
        Some(Commands::Review { number }) => commands::review(number),
        Some(Commands::Gc { force }) => commands::gc(force),
        Some(Commands::Update) => commands::update(),
        None => {
            // Check if config exists AND we're in a git repo - if so, show dashboard
            let config_path = crate::config::Config::path()?;
            let in_git_repo = git::get_root(None).is_ok();
            
            if config_path.exists() && in_git_repo {
                // Config exists and in git repo - show dashboard with session picker
                match onboarding::run_dashboard()? {
                    onboarding::DashboardResult::OpenSession(path) => {
                        commands::open(Some(path))
                    }
                    onboarding::DashboardResult::Quit => Ok(()),
                }
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
