.PHONY: build install install-deps install-config uninstall clean doctor

PREFIX ?= $(HOME)/bin
CARGO_TARGET ?= release

# Build the CLI
build:
	cargo build --release

# Full install: binary + dependencies + config
install: build install-deps install-config
	@mkdir -p $(PREFIX)
	@cp target/release/ws $(PREFIX)/ws
	@chmod +x $(PREFIX)/ws
	@echo ""
	@echo "✓ Installed ws to $(PREFIX)/ws"
	@echo ""
	@echo "Make sure $(PREFIX) is in your PATH"
	@echo "Run 'ws doctor' to verify all dependencies"

# Install just the binary (no deps/config)
install-bin: build
	@mkdir -p $(PREFIX)
	@cp target/release/ws $(PREFIX)/ws
	@chmod +x $(PREFIX)/ws
	@echo "✓ Installed ws to $(PREFIX)/ws"

# Install only dependencies via Homebrew
install-deps:
	@echo "Installing dependencies..."
	@which brew > /dev/null || (echo "Error: Homebrew required. Install from https://brew.sh" && exit 1)
	@brew install tmux fzf lazygit 2>/dev/null || true
	@echo ""
	@echo "Note: 'droid' (Claude Code) must be installed manually from https://claude.ai/download"

# Install config files (lazygit, tmux)
install-config:
	@echo "Setting up configuration..."
	@# Lazygit config
	@mkdir -p $(HOME)/.config/lazygit
	@if [ ! -f $(HOME)/.config/lazygit/config.yml ]; then \
		cp config/lazygit.yml $(HOME)/.config/lazygit/config.yml; \
		echo "  ✓ Created lazygit config"; \
	elif ! grep -q "ws select" $(HOME)/.config/lazygit/config.yml 2>/dev/null; then \
		cat config/lazygit.yml >> $(HOME)/.config/lazygit/config.yml; \
		echo "  ✓ Added ws integration to lazygit config"; \
	else \
		echo "  ○ Lazygit config already has ws integration"; \
	fi
	@# Tmux config
	@if ! grep -q "ws select" $(HOME)/.tmux.conf 2>/dev/null; then \
		echo "" >> $(HOME)/.tmux.conf; \
		cat config/tmux.conf >> $(HOME)/.tmux.conf; \
		echo "  ✓ Added ws keybinding to tmux config"; \
	else \
		echo "  ○ Tmux config already has ws keybinding"; \
	fi
	@echo ""
	@echo "Reload tmux config: tmux source-file ~/.tmux.conf"

# Uninstall ws binary
uninstall:
	@rm -f $(PREFIX)/ws
	@echo "✓ Uninstalled ws"

# Clean build artifacts
clean:
	cargo clean

# Run doctor to check dependencies
doctor: build
	@target/release/ws doctor

# Development build
dev:
	cargo build

# Run tests
test:
	cargo test

# Format code
fmt:
	cargo fmt

# Lint code
lint:
	cargo clippy
