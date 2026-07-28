+++
title = "Sanitize the claude-code capture fixtures — real session identity is on the public repo"
created = 1785218992
updated = 1785218992
priority = 5
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
The two claude-code fixtures are verbatim `claude -p --output-format stream-json` captures off a live local session, committed in f2bdea8 and pushed to `origin/main` of a PUBLIC repo (github.com/mudbungie/brazen).

Files:
- tests/fixtures/claude_code_basic.ndjson
- tests/fixtures/claude_code_error_loggedout.ndjson

Leaked values (no credentials — `apiKeySource` is `none`, no token anywhere):
- operator email inside a thinking_delta: `mudbungie@gmail.com` (a NEW disclosure — public commits are authored as mudbungie@gmail.com)
- local paths: `cwd` = `/tmp/claude-1000/-home-mark-dev-lernie/<uuid>/scratchpad`, `memory_paths.auto` = `/home/u/.claude/projects/...`
- session_id + per-line uuid values
- account policy telemetry in rate_limit_event: `resetsAt`, `overageStatus: rejected`, `overageDisabledReason: org_level_disabled`

Fix: value-only scrub. Replace the leaked VALUES with neutral placeholders while
leaving the JSON grammar, key set, line count and event structure byte-identical
in shape — the fixtures test the wire grammar, not the content.

Consumers assert none of the scrubbed fields (`src/tests/claude_code_fixtures.rs`,
`src/tests/claude_code_run.rs` assert only on msg id, model, the thinking substring
`respond with "pong"`, usage 153/97, and the logged-out message), so the scrub is
test-neutral. Update the "REAL captured streams" wording in those doc comments and
in specs/claude-code.md §8 to say sanitized capture.

NOT rewriting history: already pushed to a public repo, so the blob survives in
GitHub fork/cache storage regardless; commit forward instead.