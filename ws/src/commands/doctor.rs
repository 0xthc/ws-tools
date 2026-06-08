use crate::config::Config;
use anyhow::Result;
use colored::*;
use std::process::Command;

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
        name: "gh",
        brew_name: "gh",
        description: "GitHub CLI for PR management",
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

    // Check the configured AI tool's binary (not a hardcoded one)
    let ai_tool = Config::load().unwrap_or_default().ai_tool;
    let ai_missing = which::which(ai_tool.binary()).is_err();
    let ai_status = if ai_missing {
        all_ok = false;
        "○".yellow().to_string()
    } else {
        "✓".green().to_string()
    };
    println!(
        "  {} {} {}",
        ai_status,
        ai_tool.binary(),
        "(AI tool, optional)".dimmed()
    );
    println!("    {}", ai_tool.name().dimmed());

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

        // The AI tool isn't installed via Homebrew - point at its installer
        if ai_missing {
            println!(
                "{} Note: {} ({}) must be installed manually:",
                "::".yellow().bold(),
                ai_tool.name(),
                ai_tool.binary()
            );
            println!("  {}", ai_tool.install_hint());
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
