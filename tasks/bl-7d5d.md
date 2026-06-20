+++
title = "Simulated provider HTTP server + end-to-end CI conformance: drive the real bz binary over the real HttpTransport (ureq) against a localhost server that replays the golden wire fixtures — no real providers/keys"
created = 1781931560
updated = 1781931673
claimant = "Dialectic"
priority = 1
tags = ["impl"]
+++
User: 'implement full ci/cd ... including the simulated providers http server (not real providers obviously).' Fills the one gap MockTransport can't: the REAL HTTP path (ureq HttpTransport) end to end.

## Design
- tests/sim_support/mod.rs — FakeProvider: bind 127.0.0.1:0 in a daemon thread; for ANY request, drain it then reply with one canned (content-type, body). base_url() returns http://127.0.0.1:PORT. ~90 lines.
- tests/sim_conformance.rs — reuse tests/live_support/exec.rs (run_bz) + grammar.rs (events/ty/want/last_is) via `#[allow(dead_code)] #[path=...] mod`. Per-provider data table: for each of anthropic/openai/openai-responses/google/ollama, start a FakeProvider serving the provider's golden basic fixture (tests/fixtures/*.sse|*.ndjson), write a temp --config with a full provider row whose base_url -> the server, run `bz --config C --provider P --model M [--api-key dummy] --json 'hi'`, assert exit 0 + canonical grammar (message_start, text content_start, text_delta, terminal). NOT #[ignore]d.
- Content-types: text/event-stream for .sse; application/x-ndjson for ollama .ndjson. auth=none (ollama) omits --api-key.

## CI
Runs in the EXISTING jobs: ci.yml matrix (`cargo test --workspace` on all 7 targets — real HTTP path proven cross-platform incl. Windows/musl) and the gate (make check -> llvm-cov runs tests). No new secret. Verify the exact terminal event type empirically by running locally before asserting.

## Spec
architecture.md §9/§10: add the 'simulated-provider conformance' tier (offline, real-transport e2e) alongside the live suite.

## Close gate
`cargo test --test sim_conformance` passes locally; make check green (sim tests don't affect lib coverage — test code isn't lib lines, and the bz subprocess is coverage-excluded).