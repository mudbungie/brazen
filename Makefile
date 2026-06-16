.PHONY: help hooks build test cov fmt fmt-check lint check clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n",$$1,$$2}'

hooks: ## Enable the repo pre-commit gate (run once per clone)
	git config core.hooksPath .githooks
	@echo "pre-commit gate enabled (.githooks)"

build: ## Build the workspace
	cargo build --workspace

test: ## Run tests
	cargo test --workspace

cov: ## Enforce 100% line coverage (bz shim crate excluded — see specs §9.5)
	cargo llvm-cov --fail-under-lines 100 --ignore-filename-regex 'bz/'

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Verify formatting
	cargo fmt --all --check

lint: ## Clippy with warnings as errors (whole workspace, incl. the bz shim)
	cargo clippy --workspace --all-targets -- -D warnings

check: fmt-check lint cov ## Full gate: format + lint + 100% coverage

clean: ## Remove build artifacts
	cargo clean
