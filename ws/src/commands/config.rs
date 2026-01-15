use super::workspace::open;
use crate::config::{AiTool, Config};
use crate::onboarding;
use anyhow::{Context, Result};
use colored::*;

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
