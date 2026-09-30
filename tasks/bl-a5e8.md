+++
title = "pre-commit gate: delegate make check to the noodlezoo builder (bl-remote-gate); no local build path"
created = 1790733819
updated = 1790733881
claimant = "Junketing-brazen"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Ops tracking: ~/ops bl-3e3f. Reference: thrall scripts/pre-commit; design ~/ops/remote-builds.md 'Repo gate'.

Shape: leak-scan locally -> export BALLS_TOOLCHAIN -> bl-speculate check (hit => 0) -> bl-remote-gate (its exit is the gate's). The builder runs `make check` in its image.

Folds: rust-toolchain.toml (the image has NO default toolchain; the pin is the verdict key's toolchain half on both sides), CI RUSTUP_TOOLCHAIN where a job must NOT use the pin (msrv, matrix).

STATUS 2026-09-29: worktree staged, first commit attempted. Builder ran make check: fmt-check, clippy, linecount, leak-scan all pass on the builder; `make cov` fails with 'no such command: llvm-cov' — the builder image ships cargo-tarpaulin 0.35.2, not cargo-llvm-cov. Builder verdict = fail (exit 1, verified). BLOCKED on a decision: swap `make cov` to tarpaulin (thrall shape: --engine llvm --fail-under 100, --exclude-files for src/main.rs, src/native, src/tests; touches Makefile, ci.yml gate job, AGENTS.md, specs/architecture.md:1257) or add cargo-llvm-cov to the image. Not decided here.