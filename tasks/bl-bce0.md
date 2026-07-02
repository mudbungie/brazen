+++
title = "Thickness pass: bring >200-line source files under 200 via real seams (excl. canonical/anthropic/google lanes)"
created = 1782971919
updated = 1782972118
claimant = "applicants-slim"
priority = 20
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["refactor"]
+++
# Thickness pass: bring >200-line source files under 200 via real seams

## Mandate (user, 2026-07-01)
300 is the hard cap that prevents runaway; **200 is the thickness indicator** — a file over it is probably thick and should be made tighter. Refactor the files below to sit under 200 lines each, via GENUINE seams, never token-shuffling or shallow indirection. ZERO behavior change: wire bytes, CLI output, exit codes, and the public API are identical; existing tests keep passing (only `use` paths may move).

## Scope — exactly these files (line counts at filing)
```
293 src/run/mod.rs
271 src/run/events.rs
270 src/run/models.rs
262 src/auth/mod.rs
256 src/protocol/openai_responses/decode/mod.rs
255 src/cli.rs
231 src/config/resolve.rs
220 src/protocol/ollama_chat/encode.rs
209 src/config/partial.rs
207 src/store.rs
207 src/native/tests.rs
201 src/native/transport.rs
```

## Hard exclusions — parallel lanes own these; do NOT touch
- `src/canonical/**`, `src/protocol/anthropic/**`, `src/protocol/google_genai/**` (two concurrent feature lanes).
- `src/lib.rs` (a parallel lane edits it — prefer NESTED submodules, `foo.rs` → `foo/mod.rs` + parts, so the top-level `mod` list never changes).
- `src/tests/**` except mechanical `use`-path fixes your moves force. (`src/native/tests.rs` IS in scope — it is not under `src/tests/`.)
- `CHANGELOG.md` (no user-visible change → no entry). Specs: only `specs/architecture.md` §11 (module layout) — update it for EVERY module you add; the layout doc is authoritative.
- `event.rs` (299) and `request.rs` (247) look thick but belong to the feature lanes — leave them.

## Method
- Follow the established sibling-module precedent (`request.rs`/`request_de.rs`; a parallel lane adds `event.rs`/`event_serde.rs` the same way): split along data-vs-serde, verb-vs-plumbing, or per-concern seams where each half reads as one idea.
- Modules stay PRIVATE with hand re-exports (arch §9.8 parity: the public surface must not change — `tests/interface_parity.rs` enforces both directions).
- While splitting, tighten: delete redundant paths and dead words you find; do not "fix" behavior — if you find a real bug, note it in the close message for a follow-up ball instead.
- If a file resists a real seam (a genuinely cohesive 210-liner), tighten it; if it still exceeds 200, LEAVE IT and note it in the close message rather than inventing a fake seam. Sub-200 is an indicator, not a gate; the gate stays 300.

## Conventions — MUST follow (repo AGENTS.md + commit gate)
- **Worktree discipline.** `bl claim <id>` prints a worktree path on stdout. `cd` into it. ALL edits happen there — NEVER edit the repo core working tree.
- **Commit gate (`bl close` runs it).** The local pre-commit enforces: identity `mudbungie <mudbungie@gmail.com>`; NO commit timestamped in the weekday 09:00–17:00 America/Los_Angeles window (if committing during that window, export a synthetic evening `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` for the session); `make check` = fmt-check + clippy `-D warnings` + linecount (no tracked `*.rs` > 300) + `cargo llvm-cov --fail-under-lines 100`.
- **Workflow:** edit in the worktree → `make check` green → commit with the `[bl-id]` tag → `bl close <id>` from the REPO ROOT `/home/mark/dev/brazen` (never from inside the worktree). If close aborts on a main-fold conflict: merge `main` into the worktree by hand, resolve, re-run `make check`, close again.
- **Never credit AI or tooling in commit messages.**
- Two feature lanes close into `main` concurrently: your files are disjoint from theirs, but the fold can still race — retry a failed close once; a concurrent close can spuriously break a parallel `cargo test` run — re-run before diagnosing. Consider closing AFTER a `git -C /home/mark/dev/brazen pull --ff-only` shows main quiet.