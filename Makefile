.PHONY: help hooks build test cov fmt fmt-check lint linecount check smoke clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n",$$1,$$2}'

hooks: ## Enable the repo pre-commit gate (run once per clone)
	git config core.hooksPath .githooks
	@echo "pre-commit gate enabled (.githooks)"

build: ## Build the workspace
	cargo build --workspace

test: ## Run tests
	cargo test --workspace

cov: ## Enforce 100% line coverage (bz bin + src/native shim excluded — see specs §9.5)
	cargo llvm-cov --fail-under-lines 100 --ignore-filename-regex 'src/(main\.rs|native)'

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Verify formatting
	cargo fmt --all --check

lint: ## Clippy with warnings as errors (lib + the bz bin, all targets)
	cargo clippy --workspace --all-targets -- -D warnings

linecount: ## No tracked *.rs exceeds 300 lines (docs/config exempt); repo-wide
	@cap=300; fail=0; \
	for f in $$(git ls-files '*.rs'); do \
		n=$$(wc -l <"$$f" | tr -d ' '); \
		if [ "$$n" -gt "$$cap" ]; then \
			echo "linecount: $$f is $$n lines (> $$cap-line cap for code files)" >&2; \
			fail=1; \
		fi; \
	done; \
	[ "$$fail" -eq 0 ] || { echo "linecount: code files exceed the $$cap-line cap" >&2; exit 1; }

check: fmt-check lint linecount cov ## Full gate: format + lint + 300-line cap + 100% coverage

smoke: build ## Live smoke test per provider (needs real keys; skips absent ones)
	BZ=target/debug/bz scripts/smoke.sh

clean: ## Remove build artifacts
	cargo clean
