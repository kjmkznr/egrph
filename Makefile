.PHONY: all build test setup check-wasm-pack \
        build-wasm build-wasm-web build-wasm-node \
        test-wasm test-wasm-node demo clean publish-wasm

# PATH that prefers rustup-managed toolchain (required for WASM targets)
RUSTUP_CARGO_BIN := $(HOME)/.cargo/bin
export PATH := $(RUSTUP_CARGO_BIN):$(PATH)

all: build

# Build all native crates
build:
	cargo build --release

# Run all native tests (excludes egrph-wasm which requires wasm-pack)
test:
	cargo test --workspace --exclude egrph-wasm

# ---------------------------------------------------------------------------
# One-time setup: install rustup + wasm32 target
# (required when using Homebrew Rust, which lacks the wasm32 sysroot)
# ---------------------------------------------------------------------------
setup:
	@echo "==> Checking rustup..."
	@if ! command -v rustup > /dev/null 2>&1; then \
		if command -v brew > /dev/null 2>&1; then \
			echo "==> Installing rustup via Homebrew..."; \
			brew install rustup; \
			$$(brew --prefix rustup)/bin/rustup-init -y --no-modify-path --default-toolchain stable; \
		else \
			echo "==> Installing rustup via rustup.rs..."; \
			# NOTE: curl|sh carries supply-chain risk even over HTTPS. \
			# To verify manually instead, visit https://rustup.rs and follow \
			# the instructions, then re-run this target. \
			curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain stable; \
		fi; \
		echo "==> rustup installed. PATH update may be needed: source ~/.cargo/env"; \
	else \
		echo "==> rustup already available."; \
	fi
	@echo "==> Adding wasm32-unknown-unknown target..."
	@$(HOME)/.cargo/bin/rustup target add wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
	@echo "==> Checking wasm-pack..."
	@if ! command -v wasm-pack > /dev/null 2>&1; then \
		echo "==> Installing wasm-pack..."; \
		cargo install wasm-pack; \
	else \
		echo "==> wasm-pack already available."; \
	fi
	@echo ""
	@echo "Setup complete. Run: make build-wasm"

# ---------------------------------------------------------------------------
# WASM targets
# ---------------------------------------------------------------------------

check-wasm-pack:
	@which wasm-pack > /dev/null 2>&1 || \
		(echo "Error: wasm-pack not found. Run: make setup" && exit 1)
	@if command -v rustup > /dev/null 2>&1; then \
		rustup target list --installed | grep -q wasm32-unknown-unknown || \
		(echo "Error: wasm32-unknown-unknown not installed. Run: make setup" && exit 1); \
	else \
		echo "Warning: rustup not found; skipping wasm32 target check (build will fail if target is absent)"; \
	fi

# Bundler target (webpack / vite / rollup) — output: egrph-wasm/pkg/
build-wasm: check-wasm-pack
	wasm-pack build egrph-wasm \
		--release \
		--target bundler \
		--out-dir pkg \
		--out-name egrph_wasm \
		--scope kjmkznr

publish-wasm: build-wasm
	cd egrph-wasm/pkg && npm publish

# Web target (direct browser ESM, no bundler) — output: egrph-wasm/pkg-web/
build-wasm-web: check-wasm-pack
	wasm-pack build egrph-wasm \
		--release \
		--target web \
		--out-dir pkg-web \
		--out-name egrph_wasm

# Node.js target (CommonJS) — output: egrph-wasm/pkg-node/
build-wasm-node: check-wasm-pack
	wasm-pack build egrph-wasm \
		--release \
		--target nodejs \
		--out-dir pkg-node \
		--out-name egrph_wasm

# Run WASM tests in Node.js (no browser required).
# Prepends ~/.volta/bin so wasm-bindgen-test-runner can find node when managed
# by Volta. nvm and system-installed node are expected to already be in PATH
# when make is invoked (nvm sets up PATH in the shell profile).
test-wasm-node: check-wasm-pack
	PATH="$(HOME)/.volta/bin:$(PATH)" wasm-pack test egrph-wasm --node

# Run WASM tests in headless Chrome (requires chromedriver)
test-wasm: check-wasm-pack
	wasm-pack test egrph-wasm --headless --chrome

# Build WASM (web target) and launch demo server at http://localhost:8080/demo/
demo: check-wasm-pack
	wasm-pack build egrph-wasm \
		--release \
		--target web \
		--out-dir pkg \
		--out-name egrph_wasm
	@echo ""
	@echo "==> Open http://localhost:8080/demo/ in your browser"
	cd egrph-wasm && python3 -m http.server 8080

clean:
	cargo clean
	rm -rf egrph-wasm/pkg egrph-wasm/pkg-web egrph-wasm/pkg-node
