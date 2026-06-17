+++
title = "Fuzz the OpenAI (ChatGPT-SSO) integration against the live service's expected request/stream/error behaviors"
created = 1781672837
updated = 1781674613
claimant = "Gladiolas"
tags = ["testing"]
+++
Fuzz brazen's OpenAI integration against the REAL service's expected behaviors: generate a wide range of canonical requests + malformed/edge inputs, drive them through `bz` to the live OpenAI ChatGPT-SSO provider, and assert brazen's normalization/error-mapping matches what the service actually does — surfacing where brazen mis-encodes, mis-decodes, or mis-maps errors.

## Scope (openai_responses protocol + codex-backend behaviors)
- **Request-shape fuzzing:** vary instructions/system presence, message roles/ordering, content kinds (text, images), tool defs + `tool_choice` spellings, max_tokens/temperature/top_p ranges, stream true/false, store true/false/absent, `extra` passthrough fields, unicode/large/empty inputs. Assert encode produces a body the service accepts, or that a service 4xx maps to the correct ExitClass.
- **Response/stream fuzzing:** assert the SSE state machine decodes real `response.*` event sequences into the canonical Event taxonomy (reasoning/thinking channels, output_text deltas, tool calls, usage on `response.completed`, refusals, mid-stream errors). Capture live responses as golden fixtures and feed them back through the offline decoder.
- **Error-behavior conformance** (validated live 2026-06-16): missing instructions → 400 "Instructions are required"; missing store → 400 "Store must be set to false"; stream:false → 400 "Stream must be set to true"; unsupported model (e.g. `gpt-5-codex` on a ChatGPT account) → 400 "… not supported". Working model: `gpt-5.4`.
- **Auth/refresh fuzzing:** near-expiry token → silent refresh; revoked → exit 77.

## Constraints
- **Opt-in/live only** (never in `make check`/CI; `#[ignore]` or `BRAZEN_LIVE=1`). Costs real tokens — bound request volume and LOG what was skipped/capped (no silent truncation, AGENTS.md).
- **Build on the live integ harness (sibling bl-04dc)** for provider discovery + request running rather than duplicating it.
- Any brazen bug found becomes its own ball — notably the KNOWN error-body-swallow gap (non-2xx body dropped → empty message/null provider_detail), which made this exact debugging painful.

## Context
OpenAI ChatGPT-SSO landed in auth §10 (commits 7beffef design, bccc73b impl); the §10.7 risk list is now validated live. Provider row `openai-chatgpt` over `openai_responses` against `https://chatgpt.com/backend-api/codex`.