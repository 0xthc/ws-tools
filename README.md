# ws-tools

A Cargo workspace containing CLI tools for terminal-based development workflows.

## Crates

| Crate | Description |
|-------|-------------|
| [ws](ws/) | Workspace CLI for git worktrees with tmux layouts |
| [texplore](texplore/) | Terminal file explorer with git integration |

## ws Commands

### Workspace Management

| Command | Alias | Description |
|---------|-------|-------------|
| `ws` | | Interactive dashboard to select and open worktrees (runs onboarding on first use) |
| `ws open [target]` | `o` | Open workspace for a directory, branch, or worktree name |
| `ws new <branch> [--from <base>]` | `n` | Create new worktree from base branch and open workspace |
| `ws select` | `s` | Interactive worktree selector using fzf |
| `ws delete <target> [--force]` | `d`, `rm` | Delete worktree, tmux session, and local branch |
| `ws reload [target]` | `r` | Kill and recreate tmux session with current config |
| `ws status` | | Interactive status dashboard showing worktrees and sessions |
| `ws sync [--create] [--delete]` | | Sync tmux sessions with worktrees, clean up orphans |

### Git Workflow

| Command | Alias | Description |
|---------|-------|-------------|
| `ws clone <url>` | `c` | Clone repository and set up workspace structure |
| `ws pr` | | Create a pull request from current worktree (opens in browser) |
| `ws pr list` | | List PRs for branches with worktrees |
| `ws review <number>` | | Checkout PR into a new worktree for review |
| `ws gc [--force]` | | Garbage collect merged branches and their worktrees |

### Configuration

| Command | Alias | Description |
|---------|-------|-------------|
| `ws ai [tool]` | `a` | Switch AI tool in current tmux session (TUI selector if no arg) |
| `ws config [key] [value]` | | View or set configuration values |
| `ws init` | | Re-run setup wizard (backs up existing config) |
| `ws doctor [--install]` | | Check dependencies, optionally install with Homebrew |
| `ws update` | | Update ws and texplore via Homebrew |

## Installation

### From Source

```bash
git clone https://github.com/0xthc/ws-tools.git
cd ws-tools
cargo build --release

# Install both binaries
cp target/release/ws ~/bin/
cp target/release/texplore ~/bin/
```

## Building

```bash
# Build all crates
cargo build --release

# Build specific crate
cargo build --release -p ws
cargo build --release -p texplore
```

## Development

```bash
# Run ws
cargo run -p ws -- --help

# Run texplore
cargo run -p texplore
```
