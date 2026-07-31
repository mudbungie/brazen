+++
title = "Design an explicit release process — deliverable: specs/release.md"
created = 1785200961
updated = 1785474348
claimant = "Sulfates-ee5d"
priority = 4
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design", "release"]
+++
There is release AUTOMATION but no release PROCESS: no document says what must be
true before a version ships, and the checks that would prove it are unrunnable by
anything but a human who remembers they exist. Owner ask, 2026-07-26: "we should
probably make an explicit release process, which covers all the bases together".

Deliverable is a tracked living document — `specs/release.md` — edited like code, not
a growing task description (AGENTS.md). The WIRING is a separate ball that needs this
one; this ball decides, that ball implements.

## What exists today

- `.github/workflows/release-plz.yml` — release PR on push to main, publish on merge,
  plus binary upload and a branch-prune job. `RELEASE_PLZ_TOKEN` is live.
- `release-plz.toml` — patch by default; `[minor]`/`[major]` as standalone bracketed
  tokens opt into a bigger bump. Changelog groups everything under "Changes".
- `.github/workflows/ci.yml` — `make check`, `cargo audit --deny warnings`, an MSRV
  build, and a per-target build/test matrix.
- `Makefile` — `check` (fmt-check + lint + linecount + cov + a native-certs
  `cargo check`), and `smoke` (`scripts/smoke.sh`, live, needs real keys).

## The hole

`make check` is the merge-to-main gate and it is exclusively OFFLINE. Every check that
touches a real provider is invisible to it:

- `tests/live_conformance.rs`, `live_encode_openai.rs`, `live_fuzz_openai.rs`,
  `live_oauth_openai.rs`, `oauth_smoke.rs`, `ollama_smoke.rs` — all `#[ignore]`d and
  `BRAZEN_LIVE`-gated; `live_fuzz_openai.rs` needs a further `BRAZEN_LIVE_FUZZ_SPEND=1`.
- Grep `BRAZEN_LIVE|--ignored|smoke|fuzz` over `.github/workflows/` → NOTHING. CI never
  runs any of it.
- There is no `make live`, no `make fuzz`. `make smoke` is the only live entry point and
  it drives `scripts/smoke.sh`, not the harnesses.

Consequence, demonstrated: bl-f90e shipped a reasoning encoder that burned thinking
tokens no caller could ever see, and bl-1ad0 found that the live case named
`reasoning-summary` — the exact test for it — has been RED (exit 64, parse error) and
unrun. Offline tests were 100% green throughout, because they asserted what the code
did rather than what the dialect requires.

Owner ruling on placement: these are NOT a normal merge-to-main gate (they cost money,
need credentials, and are model-discretion flaky) — they are a VERSION RELEASE gate.

## What the document must decide

- The ladder: which suites run at commit, at merge-to-main, and at release, and WHY
  each sits where it does. Name the env gates and credentials each rung needs.
- How a release gate runs at all given release-plz publishes from CI: a human-run
  `make release-check` before tagging, a manual `workflow_dispatch`, or a gated CI job
  with secrets. Pick one; the others become the rejected-alternatives section.
- What a live suite failing MEANS for a release — blocking, or advisory-with-signoff.
  Model-discretion flakiness (an empty reasoning summary, seen live 2026-07-26) makes a
  naive "all green or no ship" rule unlandable; decide the answer, do not discover it.
- Which providers are in scope for a release gate. Live coverage today is
  codex/ChatGPT-SSO, Anthropic, Google, Ollama, Mistral (per the `BRAZEN_LIVE_*_MODEL`
  vars) — an unconfigured provider must SKIP loudly, never silently pass.
- The human steps that already exist but are unwritten: changelog dump-curation before
  merging the release PR, and verifying the binaries actually attached to the Release.
- The version-bump decision: when `[minor]`/`[major]` is warranted, and who decides.

Attack it before committing (AGENTS.md): what does it not solve? A release gate that
needs a human with live credentials is a bus-factor of one — say so in the doc and pick
a default, not a feature. Prefer an existing explicit signal over new flags/config.

## Constraint

The document is the single source of truth for the process; the Makefile stays the
single source of truth for WHAT each target does (the pattern the main-advance hook
already follows: it decides only WHEN, `make install` decides what). Do not restate
target internals in prose that will drift.