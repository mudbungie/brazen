+++
title = "Wire the live + fuzz suites into a release gate"
created = 1785200965
updated = 1785474467
claimant = "Sulfates-3dc4"
priority = 3
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["testing", "release"]

[[blockers]]
id = "bl-ee5d"
on = "claim"

[[blockers]]
id = "bl-1ad0"
on = "claim"
+++
Implement the ladder specs/release.md decides (bl-ee5d). Blocked on that design AND on
bl-1ad0, which fixes the one live fuzz case that is currently RED — wiring an unrunnable
suite into a gate would only paint the gate red on day one.

## Starting state (surveyed 2026-07-26)

The harnesses were NOT removed; they are simply unreachable by automation:

    tests/live_conformance.rs      #[ignore] + BRAZEN_LIVE
    tests/live_encode_openai.rs    #[ignore] + BRAZEN_LIVE
    tests/live_fuzz_openai.rs      #[ignore] + BRAZEN_LIVE + BRAZEN_LIVE_FUZZ_SPEND
    tests/live_oauth_openai.rs     #[ignore] + BRAZEN_LIVE
    tests/oauth_smoke.rs           #[ignore]
    tests/ollama_smoke.rs          #[ignore]

Model selection rides `BRAZEN_LIVE_{ANTHROPIC,GOOGLE,MISTRAL,OLLAMA,OPENAI,
OPENAI_CHATGPT,OPENAI_RESPONSES}_MODEL`. `grep -rn "BRAZEN_LIVE|--ignored|smoke|fuzz"
.github/workflows/` returns NOTHING, and the Makefile has no `live` or `fuzz` target —
only `smoke`, which runs `scripts/smoke.sh`, a different thing.

Note `live_fuzz_openai.rs` is a REQUEST-SHAPE fuzzer (generated body variants driven at
a real provider), not a `cargo-fuzz`/libFuzzer target. If bl-ee5d concludes coverage-
guided fuzzing of the decoders is also wanted, that is a separate ball — do not
silently widen this one.

## Work

- Makefile targets for each rung the design names, so the entry point is one word and
  the Makefile stays the single source of truth for what running them means.
- The runner bl-ee5d picked (pre-tag `make release-check`, `workflow_dispatch`, or a
  secrets-gated CI job). Credentials come from repo secrets, never a committed config.
- Loud SKIP for an unconfigured provider — an absent credential must never read as a
  pass. That is the failure mode that hid bl-f90e.
- Whatever flakiness policy the design set (the reasoning summary is model discretion:
  one run in three printed no thinking, live 2026-07-26). Encode the policy; do not
  weaken assertions to dodge it.

## Scope

`Makefile` and `.github/workflows/` — kept disjoint from bl-1ad0 (`tests/`), bl-2704
(`src/protocol/openai_responses/`), and bl-b68b (`src/protocol/openai/`).