+++
title = "Collapse the bz shim crate into a single published `brazen` crate (lib brazen + bin bz), mirroring balls/bl; relax bl-c420 from crate-graph enforcement to a test-enforced purity boundary"
created = 1781927040
updated = 1781927095
claimant = "Dialectic"
priority = 1
tags = ["refactor"]
+++
## Goal
One published crate `brazen` whose `cargo install brazen` yields the `bz` command — the exact balls→bl pattern (single package, `[lib] name="brazen"` + `[[bin]] name="bz" path="src/main.rs"`, no workspace). The squatted `bz` crate name is thus irrelevant; the shim is never published separately.

## Why (decision)
User: 'The cargo location is brazen, the cli you get out is bz. Same as balls→bl. I'm not shipping a library; it's a compiled component.' `cargo install brazen` requires the bin to live in the `brazen` crate, so the two-crate split cannot survive: ureq/libc become crate-wide deps and the compiler can no longer forbid `use ureq` in a pure module. bl-c420's crate-graph guarantee is therefore relaxed to a TEST guarantee.

## Mechanical moves
- bz/src/main.rs → src/main.rs (bin `bz`). Keeps `use brazen::{...}` (a package's bin sees its own lib by name).
- bz/src/native.rs → src/native/mod.rs; bz/src/native/{creds,rng,cache,cache_tests,tests}.rs → src/native/. Add `mod transport; pub use transport::HttpTransport;` to native/mod.rs.
- bz/src/transport.rs → src/native/transport.rs (RENAME to avoid colliding with the lib's src/transport.rs, which is the Transport TRAIT/seam). bz/src/transport/tests.rs → src/native/transport/tests.rs. main.rs: `mod native;` only; `use native::HttpTransport`.
- bz/tests/* → tests/ (live_* harnesses; they use env!("CARGO_BIN_EXE_bz") which still resolves — bin is still named bz).
- Cargo.toml: drop [workspace]+members; add [[bin]] name=bz path=src/main.rs; add ureq (default-features=false, rustls) + [target.'cfg(unix)'.dependencies] libc; keep dev-dep tempfile. serde_json/base64 already lib deps. Delete bz/Cargo.toml.
- Makefile cov: change --ignore-filename-regex 'bz/' → 'src/(main\.rs|native/)' so the bin+native shim stays coverage-excluded while the lib reaches 100%.

## Purity re-enforcement (replaces bl-c420 crate graph)
Add tests/purity.rs: walk every lib source file under src/ EXCEPT src/main.rs and src/native/, assert none contain `ureq`/`libc`/`std::net`. Compiler-guarantee → executable test-guarantee. Seam traits (Transport/CredStore/ModelCache/Clock/BrowserLauncher/CodeReceiver) stay, so 100%-with-mocks testability is fully preserved.

## Spec amendment (fix the doc — AGENTS.md: don't implement a deviation, fix the doc)
Rewrite architecture.md §9.5 + §10 'Crate split' (lines ~881, ~906) from 'two member crates / crate graph enforces network-free' to 'single crate brazen; bin bz + src/native isolate the impurity; purity is a test invariant + coverage path-exclusion'. Sweep other 'bz crate'/'shim crate'/'two-crate' refs in specs/architecture.md, config.md, auth.md, model-discovery.md (the heavy doc reconciliation is its own task, but fix anything that would be a LIE post-collapse).

## Close gate
make check green (fmt + clippy -D + linecount + 100% lib cov via the new regex). All tests pass incl. the new purity test.