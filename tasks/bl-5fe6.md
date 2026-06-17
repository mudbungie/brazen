+++
title = "Surface the upstream non-2xx response body in CanonicalError (provider errors are currently empty / undiagnosable)"
created = 1781674386
updated = 1781674386
tags = ["bug"]
+++
brazen drops the upstream non-2xx HTTP response BODY, so provider errors are undiagnosable. On a 4xx/5xx the `CanonicalError` surfaces `kind = provider{status}` but `message = ""` and `provider_detail = null` — the actual upstream body is discarded. Live example (OpenAI codex backend): the service returned `{"detail":"Store must be set to false"}` with HTTP 400, but `bz --json` emitted `{"type":"error","kind":{"provider":{"status":400}},"message":"","provider_detail":null}`. This turned a one-field fix into a multi-step curl investigation.

## Fix
- When a non-2xx response is decoded into a provider error, capture the raw response body into `provider_detail` (and/or `message`). The body shape differs per provider, so surface the RAW body rather than trying to parse a uniform error schema.
- Applies across protocols (anthropic_messages, openai_chat, openai_responses, google_generative_ai, ollama_chat) — likely a shared whole-body-error path; `Frame.status` already carries the authoritative status (see AGENTS.md "carry the fact" note), so this is about not throwing away the bytes alongside it.
- **`--raw` also produced NOTHING on a 4xx** — raw/IdentityDecoder should arguably pass the error body through verbatim too. Investigate both the structured-error path and `--raw`.
- Mind `Secret` redaction discipline (error bodies should not contain secrets, but do not log request creds while doing this).
- Add tests (the existing `*_decode_errors.rs` suites + a whole-body 4xx case asserting the body reaches `provider_detail`). Keep 100% line coverage.

## Context
Discovered while validating OpenAI ChatGPT-SSO (auth §10). Referenced as a known gap by the live integ (bl-04dc) and fuzz (bl-b72f) balls.