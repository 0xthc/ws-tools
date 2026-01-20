# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2025-01-13

### Added
- Initial release
- `ws open` - Open workspace for a directory, branch, or worktree name
- `ws new` - Create new worktree and open workspace
- `ws select` - Interactive worktree selector with fzf
- `ws delete` - Delete worktree, tmux session, and local branch
- `ws sync` - Sync tmux sessions with worktrees
- `ws status` - TUI dashboard showing worktrees and sessions (ratatui)
- `ws doctor` - Check and install dependencies
- Ghostty tab naming with repo/worktree and branch
- tmux and lazygit integration configs
