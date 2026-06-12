# SpaceTerm — build & test orchestration.
# Run `make help` for the target list.

CMD ?= ls -la
FRONTEND_PKGS := frontend/block-renderer frontend/palette frontend/rich-renderers

UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
	ADDON_LIB := libspaceterm_bindings.dylib
else
	ADDON_LIB := libspaceterm_bindings.so
endif

.PHONY: help build test rust-test frontend-install frontend-test py-test \
        lint fmt addon vscode demo clean

help:
	@echo "spaceterm targets:"
	@echo "  make build               Build all Rust crates"
	@echo "  make test                Run every test (Rust + frontend + Python)"
	@echo "  make rust-test           Run Rust workspace tests"
	@echo "  make frontend-test       Install + test the TS packages, typecheck the ext"
	@echo "  make py-test             Run the Python client tests"
	@echo "  make lint                clippy (deny warnings) + rustfmt check"
	@echo "  make fmt                 Format Rust + TS"
	@echo "  make addon               Build the napi addon -> crates/bindings/spaceterm.node"
	@echo "  make vscode              Build the addon and typecheck the VSCode extension"
	@echo "  make demo CMD='ls -la'   Run the integrated spaceterm pipeline on a command"
	@echo "  make clean               Remove build artifacts"

build:
	cargo build --workspace

rust-test:
	cargo test --workspace

frontend-install:
	@for pkg in $(FRONTEND_PKGS); do echo "== install $$pkg =="; (cd $$pkg && pnpm install); done
	cd frontend/vscode-ext && pnpm install --ignore-scripts

frontend-test: frontend-install
	@for pkg in $(FRONTEND_PKGS); do echo "== test $$pkg =="; (cd $$pkg && pnpm test); done
	cd frontend/vscode-ext && pnpm run typecheck

py-test:
	cd clients/client-py && uv run --with pytest python -m pytest -q

test: rust-test frontend-test py-test

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all -- --check

fmt:
	cargo fmt --all
	@for pkg in $(FRONTEND_PKGS) frontend/vscode-ext; do \
		(cd $$pkg && npx --no-install prettier --write "src/**/*.ts" "test/**/*.ts" "media/**/*.ts" 2>/dev/null || true); done

addon:
	cargo build -p spaceterm-bindings
	cp target/debug/$(ADDON_LIB) crates/bindings/spaceterm.node
	@echo "addon -> crates/bindings/spaceterm.node"

vscode: addon
	cd frontend/vscode-ext && pnpm install --ignore-scripts && pnpm run typecheck

demo:
	cargo run -q -p spaceterm -- $(CMD)

clean:
	cargo clean
	@for pkg in $(FRONTEND_PKGS) frontend/vscode-ext; do rm -rf $$pkg/node_modules; done
	rm -f crates/bindings/spaceterm.node
	rm -rf clients/client-py/.venv
