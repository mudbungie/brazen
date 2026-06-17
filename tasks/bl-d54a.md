+++
title = "openai-chatgpt (Codex) also rejects temperature + top_p as 'Unsupported parameter' — the second/third field bl-73d8 waited for; add per-row unsupported-param suppression"
created = 1781679795
updated = 1781679795
priority = 35
tags = ["bug", "openai"]
+++
Validated live 2026-06-17 against https://chatgpt.com/backend-api/codex/responses (model gpt-5.4, ChatGPT account), while plumbing brazen's encode circuit (ball bl-b72f fuzz follow-up).

## Finding
The Codex backend rejects the standard OpenAI Responses sampling params. A canonical request carrying `temperature` 400s with `{"detail":"Unsupported parameter: temperature"}`; the same with `top_p` 400s `{"detail":"Unsupported parameter: top_p"}`. This is the SAME failure mode bl-73d8 found for `max_output_tokens`.

brazen's `openai_responses` encode unconditionally forwards all three when the canonical request sets them (src/protocol/openai_responses/encode.rs): `max_tokens`->`max_output_tokens`, `temperature`, `top_p`. So:
- `bz --provider openai-chatgpt --temperature 0.5 ...` -> 400, no completion.
- `bz --provider openai-chatgpt --top-p 0.5 ...` -> 400.
- `bz --provider openai-chatgpt --max-tokens N ...` -> 400 (bl-73d8).

(With bl-5fe6 now landed the user at least sees the `detail` text; before, it was an empty error.)

## Why this is now actionable
bl-73d8 explicitly deferred the fix: "A per-row data flag to drop unsupported fields (only if a SECOND field ever joins it — do not add speculatively, architecture.md §4.6 / AGENTS.md severability)." That condition is now MET: `temperature` and `top_p` are the second and third fields. The speculative-mechanism guard is lifted.

## Suggested shape (data, not branches)
A per-row datum listing the wire keys this backend cannot accept (e.g. `unsupported_body_keys = ["max_output_tokens","temperature","top_p"]` on the openai-chatgpt row), applied as a final strip in encode — the inverse of bl-74dc's `body_defaults`. Severable: removing the row datum deletes the behavior, no core edit. Confirm the encoder is the single strip site (every protocol funnels through it).

## Open question
Whether to STRIP silently or WARN. Silent strip matches "brazen normalizes to what the provider accepts"; a stderr note would tell a user their --temperature was dropped. Lean strip + (optional) one-line stderr note, decided in the ball.

## Not in scope
Other backends are unaffected (standard OpenAI /responses and chat both accept these). This is strictly the Codex row's restriction.

Repro:
  printf '%s' '{"model":"gpt-5.4","system":[{"type":"text","text":"x"}],"messages":[{"role":"user","content":[{"type":"text","text":"say ok"}]}],"stream":true,"store":false,"temperature":0.5}' | bz --provider openai-chatgpt --json