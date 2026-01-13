# ws - Workspace CLI

A CLI tool for managing git worktrees with tmux layouts. Each worktree gets its own tmux session with a pre-configured layout (lazygit, file explorer, Claude Code).

## Installation

```bash
# Clone and install with dependencies
cd ~/workspace-cli
make install

# Or install just the binary
make install-bin

# Check dependencies
ws doctor

# Install missing dependencies
ws doctor --install
```

## Usage

```bash
ws                           # Open workspace for current directory
ws open /path/to/worktree    # Open specific worktree
ws new feat/auth             # Create worktree from develop
ws new hotfix/bug -f main    # Create worktree from main
ws list                      # List all worktrees with session status
ws select                    # Interactive worktree picker (fzf)
ws delete feat/auth          # Delete worktree and its session
ws sync                      # Clean up orphaned sessions
ws doctor                    # Check dependencies
```

### Aliases

| Command | Aliases |
|---------|---------|
| `open`  | `o`     |
| `new`   | `n`     |
| `list`  | `l`, `ls` |
| `select`| `s`     |
| `delete`| `d`, `rm` |

## Integrations

### tmux

Press `prefix + W` to open the worktree selector popup.

### lazygit

In the Worktrees panel, press `o` to open the selected worktree in a new session.

## Session Naming

Sessions are named `<repo>-<branch>`:
- `basalt-main`
- `basalt-feat-auth`
- `myproject-fix-bug-123`

## Dependencies

| Tool | Required | Description |
|------|----------|-------------|
| tmux | Yes | Terminal multiplexer |
| git  | Yes | Version control |
| fzf  | Yes | Fuzzy finder |
| lazygit | No | Git TUI |
| droid | No | Claude Code CLI |

Run `ws doctor --install` to install missing dependencies via Homebrew.
