+++
title = "Live integration test suite: canonical conformance against every locally-authed provider (Ollama, OpenAI, …)"
created = 1781672812
updated = 1781674474
claimant = "Blankly"
tags = ["testing"]
+++
Build an opt-in, network-touching integration test suite that runs canonical requests against EVERY provider the local machine actually has working auth for — discovered at runtime, skipping (not failing) any provider with no usable credential — and asserts the NORMALIZED canonical behaviors.

## Why
We now have two live-authenticated providers: Ollama (keyless local) and OpenAI ChatGPT-SSO (OAuth2 via `bz login openai-chatgpt`). All lib/unit tests are 100% offline (MockTransport/fixtures); the only network-aware tests today are the `#[ignore]`d Ollama smoke test (bl-76fd) and the OFFLINE oauth_smoke harness. There is no real end-to-end conformance test. This adds one.

## Design
- **Opt-in only.** Never runs in `make check`/CI by default: `#[ignore]` (run via `cargo test -- --ignored`) or env-gated (`BRAZEN_LIVE=1`). Must NOT affect the 100% line-coverage gate (live tests excluded from coverage, like `bz/tests`).
- **Provider discovery.** For each provider in the resolved config, detect a usable credential (inline/env api_key|bearer, a stored `Cred` in the XdgCredStore, or keyless like Ollama). Skip providers with no auth and PRINT which were skipped (no silent truncation — AGENTS.md).
- **Per-provider assertions over the CANONICAL surface** (the whole point of brazen — one canonical request → normalized events across providers): basic streamed text (MessageStart/ContentStart/text deltas/FinishReason/terminated + Usage); the --text/--json/--raw projections; system/instructions; a tool round-trip where supported; error mapping (a deliberately bad request → correct ExitClass/exit code).
- **Provider quirks are DATA per row, not branches.**

## Live findings to bake in (validated 2026-06-16)
- `openai-chatgpt`: base `https://chatgpt.com/backend-api/codex`, protocol `openai_responses`, auth `oauth2`. Working model `gpt-5.4` (the `-codex` variants are gated for this ChatGPT account). The codex backend REQUIRES: non-empty `instructions` (from system), `stream: true`, and explicit `store: false` (brazen carries `store` via the `extra` flatten passthrough). Each omission → 400 with a descriptive `detail`.
- Ollama: keyless local, CPU-only systemd service (see ollama-local-testing notes); relates to bl-528a (`auth = "none"`).
- KNOWN GAP: brazen drops the non-2xx response BODY (error message empty, provider_detail null) — assert on exit codes regardless; relates to the error-body follow-up.

## Deliverable
A `tests/`/`bz/tests/` live harness + per-provider data table; README section on how to run it and add a provider; spec update if a new test seam is introduced.