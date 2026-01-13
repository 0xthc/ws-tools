use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Available AI CLI tools
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiTool {
    Droid,   // Claude Code (default)
    Claude,  // Claude CLI
    Codex,   // OpenAI Codex CLI
    Gemini,  // Google Gemini CLI
    Copilot, // GitHub Copilot CLI
}

impl AiTool {
    /// Get the command name for this tool
    pub fn command(&self) -> &'static str {
        match self {
            AiTool::Droid => "droid",
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
            AiTool::Gemini => "gemini",
            AiTool::Copilot => "gh copilot",
        }
    }

    /// Get the binary name to check for installation
    pub fn binary(&self) -> &'static str {
        match self {
            AiTool::Droid => "droid",
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
            AiTool::Gemini => "gemini",
            AiTool::Copilot => "gh",
        }
    }

    /// Get a human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            AiTool::Droid => "Claude Code (droid)",
            AiTool::Claude => "Claude CLI",
            AiTool::Codex => "OpenAI Codex CLI",
            AiTool::Gemini => "Google Gemini CLI",
            AiTool::Copilot => "GitHub Copilot CLI",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "droid" | "claude-code" => Some(AiTool::Droid),
            "claude" => Some(AiTool::Claude),
            "codex" => Some(AiTool::Codex),
            "gemini" => Some(AiTool::Gemini),
            "copilot" | "gh-copilot" => Some(AiTool::Copilot),
            _ => None,
        }
    }

    /// Get all available tools
    pub fn all() -> &'static [AiTool] {
        &[
            AiTool::Droid,
            AiTool::Claude,
            AiTool::Codex,
            AiTool::Gemini,
            AiTool::Copilot,
        ]
    }
}

impl std::fmt::Display for AiTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command())
    }
}

/// Application configuration
#[derive(Debug)]
pub struct Config {
    pub ai_tool: AiTool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai_tool: AiTool::Droid,
        }
    }
}

impl Config {
    /// Get the config file path
    pub fn path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("ws");
        Ok(config_dir.join("config.toml"))
    }

    /// Load config from file, or return defaults
    pub fn load() -> Result<Self> {
        let path = Self::path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        let mut config = Self::default();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');

                if key == "ai_tool" {
                    if let Some(tool) = AiTool::from_str(value) {
                        config.ai_tool = tool;
                    }
                }
            }
        }

        Ok(config)
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;

        // Create config directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let content = format!(
            "# Workspace CLI Configuration\n\n# AI tool to launch in the main panel\n# Options: droid (default), claude, codex, gemini, copilot\nai_tool = \"{}\"\n",
            self.ai_tool
        );

        fs::write(&path, content).context("Failed to write config file")?;
        Ok(())
    }

    /// Check if the configured AI tool is installed
    pub fn is_ai_tool_installed(&self) -> bool {
        which::which(self.ai_tool.binary()).is_ok()
    }
}
