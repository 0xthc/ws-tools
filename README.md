# ws-tools

A Cargo workspace containing CLI tools for terminal-based development workflows.

## Crates

| Crate | Description |
|-------|-------------|
| [ws](ws/) | Workspace CLI for git worktrees with tmux layouts |
| [texplore](texplore/) | Terminal file explorer with git integration |

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
