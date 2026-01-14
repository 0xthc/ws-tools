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

        /// Base branch to create from (default: develop)
        #[arg(short, long, default_value = "develop")]
        from: String,
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
        Some(Commands::New { branch, from }) => commands::new(&branch, &from),
        Some(Commands::List) => commands::list(),
        Some(Commands::Select { path }) => commands::select(path),
        Some(Commands::Delete { target, force }) => commands::delete(&target, force),
        Some(Commands::Sync { create, delete }) => commands::sync(create, delete),
        Some(Commands::Doctor { install }) => commands::doctor(install),
        Some(Commands::Status) => commands::status(),
        Some(Commands::Config { key, value }) => commands::config(key, value),
        Some(Commands::Init) => commands::init(),
        Some(Commands::Ai { tool }) => commands::ai(tool),
        None => {
            // Default to open current directory
            commands::open(None)
        }
    }
}
