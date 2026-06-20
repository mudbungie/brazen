+++
title = "CI supply-chain gate: add cargo-audit (RUSTSEC advisories) to the ci.yml gate for a credential-handling, network-facing tool"
created = 1781927086
updated = 1781927086
priority = 5
tags = ["impl"]

[[blockers]]
id = "bl-c1e2"
on = "claim"
+++
Nice-to-have, depends on collapse (bl-c1e2). Audit is clean today (94 deps, exit 0) but nothing stops a future RUSTSEC-flagged transitive (ring/rustls/serde/toml line) from shipping in a release. Add `cargo audit --deny warnings` (rustsec/audit-check or cargo install cargo-audit --locked) as a step in the ci.yml gate job so a vulnerable Cargo.lock fails before a release. Optionally deny.toml + cargo-deny for a license allowlist. Close gate: ci.yml parses; cargo audit green on current lockfile.