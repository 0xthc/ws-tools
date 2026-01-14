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
            AiTool::Droid => "Factory AI (droid)",
            AiTool::Claude => "Claude Code",
            AiTool::Codex => "OpenAI Codex CLI",
            AiTool::Gemini => "Google Gemini CLI",
            AiTool::Copilot => "GitHub Copilot CLI",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "droid" | "factory" => Some(AiTool::Droid),
            "claude" | "claude-code" => Some(AiTool::Claude),
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

/// Available git TUI tools
#[derive(Debug, Clone, PartialEq)]
pub enum GitTool {
    Lazygit,
    Gitui,
    Tig,
    Custom(String),
}

impl GitTool {
    pub fn command(&self) -> &str {
        match self {
            GitTool::Lazygit => "lazygit",
            GitTool::Gitui => "gitui",
            GitTool::Tig => "tig",
            GitTool::Custom(cmd) => cmd,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            GitTool::Lazygit => "lazygit",
            GitTool::Gitui => "gitui",
            GitTool::Tig => "tig",
            GitTool::Custom(_) => "custom",
        }
    }

    pub fn binary(&self) -> &str {
        match self {
            GitTool::Lazygit => "lazygit",
            GitTool::Gitui => "gitui",
            GitTool::Tig => "tig",
            GitTool::Custom(cmd) => cmd.split_whitespace().next().unwrap_or(cmd),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "lazygit" => GitTool::Lazygit,
            "gitui" => GitTool::Gitui,
            "tig" => GitTool::Tig,
            _ => GitTool::Custom(s.to_string()),
        }
    }

    pub fn all() -> &'static [GitTool] {
        &[GitTool::Lazygit, GitTool::Gitui, GitTool::Tig]
    }
}

impl std::fmt::Display for GitTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command())
    }
}

/// Available file explorer tools
#[derive(Debug, Clone, PartialEq)]
pub enum ExplorerTool {
    Texplore,
    Yazi,
    Ranger,
    Lf,
    Nnn,
    Custom(String),
}

impl ExplorerTool {
    pub fn command(&self) -> &str {
        match self {
            ExplorerTool::Texplore => "texplore",
            ExplorerTool::Yazi => "yazi",
            ExplorerTool::Ranger => "ranger",
            ExplorerTool::Lf => "lf",
            ExplorerTool::Nnn => "nnn",
            ExplorerTool::Custom(cmd) => cmd,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ExplorerTool::Texplore => "texplore",
            ExplorerTool::Yazi => "yazi",
            ExplorerTool::Ranger => "ranger",
            ExplorerTool::Lf => "lf",
            ExplorerTool::Nnn => "nnn",
            ExplorerTool::Custom(_) => "custom",
        }
    }

    pub fn binary(&self) -> &str {
        match self {
            ExplorerTool::Texplore => "texplore",
            ExplorerTool::Yazi => "yazi",
            ExplorerTool::Ranger => "ranger",
            ExplorerTool::Lf => "lf",
            ExplorerTool::Nnn => "nnn",
            ExplorerTool::Custom(cmd) => cmd.split_whitespace().next().unwrap_or(cmd),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "texplore" => ExplorerTool::Texplore,
            "yazi" => ExplorerTool::Yazi,
            "ranger" => ExplorerTool::Ranger,
            "lf" => ExplorerTool::Lf,
            "nnn" => ExplorerTool::Nnn,
            _ => ExplorerTool::Custom(s.to_string()),
        }
    }

    pub fn all() -> &'static [ExplorerTool] {
        &[
            ExplorerTool::Texplore,
            ExplorerTool::Yazi,
            ExplorerTool::Ranger,
            ExplorerTool::Lf,
            ExplorerTool::Nnn,
        ]
    }
}

impl std::fmt::Display for ExplorerTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command())
    }
}

/// Application configuration
#[derive(Debug)]
pub struct Config {
    pub ai_tool: AiTool,
    pub git_tool: GitTool,
    pub explorer_tool: ExplorerTool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai_tool: AiTool::Droid,
            git_tool: GitTool::Lazygit,
            explorer_tool: ExplorerTool::Texplore,
        }
    }
}

impl Config {
    /// Get the config file path (~/.ws/config.toml)
    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(home.join(".ws").join("config.toml"))
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

                match key {
                    "ai_tool" => {
                        if let Some(tool) = AiTool::from_str(value) {
                            config.ai_tool = tool;
                        }
                    }
                    "git_tool" => {
                        config.git_tool = GitTool::from_str(value);
                    }
                    "explorer_tool" => {
                        config.explorer_tool = ExplorerTool::from_str(value);
                    }
                    _ => {}
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
            r#"# Workspace CLI Configuration

# AI tool for the main coding panel
# Options: droid (default), claude, codex, gemini, copilot
ai_tool = "{}"

# Git TUI for the top-left panel
# Options: lazygit (default), gitui, tig, or any custom command
git_tool = "{}"

# File explorer for the bottom-left panel
# Options: texplore (default), yazi, ranger, lf, nnn, or any custom command
explorer_tool = "{}"
"#,
            self.ai_tool, self.git_tool, self.explorer_tool
        );

        fs::write(&path, content).context("Failed to write config file")?;
        Ok(())
    }

    /// Check if the configured AI tool is installed
    pub fn is_ai_tool_installed(&self) -> bool {
        which::which(self.ai_tool.binary()).is_ok()
    }

    /// Check if the configured git tool is installed
    pub fn is_git_tool_installed(&self) -> bool {
        which::which(self.git_tool.binary()).is_ok()
    }

    /// Check if the configured explorer tool is installed
    pub fn is_explorer_tool_installed(&self) -> bool {
        which::which(self.explorer_tool.binary()).is_ok()
    }
}
