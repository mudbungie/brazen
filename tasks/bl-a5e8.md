+++
title = "pre-commit gate: delegate make check to the noodlezoo builder (bl-remote-gate); no local build path"
created = 1790733819
updated = 1790733819
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Ops tracking: ~/ops bl-3e3f. Reference: thrall scripts/pre-commit; design ~/ops/remote-builds.md 'Repo gate'.

Shape: leak-scan locally -> export BALLS_TOOLCHAIN -> bl-speculate check (hit => 0) -> bl-remote-gate (its exit is the gate's). The builder runs `make check` in its image (rustup + repo rust-toolchain.toml, sccache, mold, tarpaulin 0.35.2, cargo-deny, ast-grep, shellcheck, jq, python3; no cargo-llvm-cov).

Folds: rust-toolchain.toml (the image has NO default toolchain; the pin is the verdict key's toolchain half on both sides), CI RUSTUP_TOOLCHAIN where a job must NOT use the pin (msrv, matrix).