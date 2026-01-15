use anyhow::{Context, Result};
use colored::*;
use std::process::Command;

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
