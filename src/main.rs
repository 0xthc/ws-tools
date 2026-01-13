mod commands;
mod git;
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
    /// Open workspace for a directory (default: current dir)
    #[command(alias = "o")]
    Open {
        /// Path to worktree or git repository
        path: Option<PathBuf>,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Open { path }) => commands::open(path),
        Some(Commands::New { branch, from }) => commands::new(&branch, &from),
        Some(Commands::List) => commands::list(),
        Some(Commands::Select { path }) => commands::select(path),
        Some(Commands::Delete { target, force }) => commands::delete(&target, force),
        Some(Commands::Sync { create, delete }) => commands::sync(create, delete),
        Some(Commands::Doctor { install }) => commands::doctor(install),
        Some(Commands::Status) => commands::status(),
        None => {
            // Default to open current directory
            commands::open(None)
        }
    }
}
