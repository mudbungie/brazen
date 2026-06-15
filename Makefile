.PHONY: help hooks build test cov fmt fmt-check lint check clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n",$$1,$$2}'

hooks: ## Enable the repo pre-commit gate (run once per clone)
	git config core.hooksPath .githooks
	@echo "pre-commit gate enabled (.githooks)"

build: ## Build the workspace
	cargo build

test: ## Run tests
	cargo test

cov: ## Enforce 100% line coverage
	cargo llvm-cov --fail-under-lines 100

fmt: ## Format the code
	cargo fmt

fmt-check: ## Verify formatting
	cargo fmt --check

lint: ## Clippy with warnings as errors
	cargo clippy --all-targets -- -D warnings

check: fmt-check lint cov ## Full gate: format + lint + 100% coverage

clean: ## Remove build artifacts
	cargo clean
