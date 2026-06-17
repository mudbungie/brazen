+++
title = "fill_absent drops the stream flag: SSE providers never get stream:true, so every live 2xx is 'premature upstream EOF'"
created = 1781673848
updated = 1781673849
claimant = "Worded"
priority = 1
tags = ["bug"]
+++
## Symptom
A live request to any SSE provider (anthropic/openai/google/mistral/openai-responses) returns exit 69 `premature upstream EOF`. Ollama is unaffected (NDJSON framer tolerates a single-object body).

## Root cause
- `--stream` / config `stream` resolves into `ResolvedConfig.stream` but `fill_absent()` (src/config/resolve.rs) never copies it into `CanonicalRequest.stream`. It fills model/max_tokens/temperature/top_p/system only.
- A positional-prompt request is built `..Default::default()` → `req.stream = false`.
- `encode` sends `"stream": false` → Anthropic returns a single JSON message.
- `drive()` (src/run/respond.rs) always SSE-frames a 2xx → no `event:`/`data:` frames → terminal marker never seen → premature EOF. (Non-2xx works because it uses whole_body drain+decode.)

## Proof
Canonical request via stdin with `"stream": true` → `ok`, exit 0. Verified live against api.anthropic.com (OAuth bearer override).

## Fix
Propagate the resolved stream into the request in fill_absent, like the other gen fields. Note design wrinkle: `req.stream` is a bare bool (no 'absent' state) — a stdin canonical request can't express explicit-false; decide whether to keep bool-or semantics or widen.

## Done when
- fill_absent propagates stream; `bz --stream <prompt>` against an SSE provider streams cleanly.
- Regression test in the lib (pure, via fill_absent / encode) pinning stream propagation.
- make check green (fmt + clippy -D + 100% coverage).
- specs/config.md §4 fill_absent field list updated.