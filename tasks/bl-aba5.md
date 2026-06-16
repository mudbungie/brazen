+++
title = "Validate responses/google/ollama/mistral wire shapes against live APIs (fixtures are hand-authored)"
created = 1781652070
updated = 1781652070
priority = 40
tags = ["test"]
+++
The golden fixtures for the four providers landed in bl-2f13 were hand-authored from API knowledge, NOT captured from live servers. A few encode/decode shapes are educated guesses that should be confirmed against real endpoints before these providers are trusted:
- google_genai encode: `functionResponse.response` is emitted as {"result": <text>} (a guess at Google's expected object shape) — src/protocol/google_genai/encode.rs, fn function_response.
- ollama_chat: tool-call `arguments` decoded as a JSON object and re-emitted; request-side images as a bare base64 array — confirm against the live /api/chat wire.
- openai_responses: the `response.*` event field names (output_index/content_index/item.call_id/usage.input_tokens_details.cached_tokens) — confirm against the Responses streaming API.
- mistral: confirm the openai_chat dialect is honored verbatim (tool_choice 'any'/'required', no max_tokens).

Suggested: one live smoke test per provider (a real key, a tiny request, assert a clean Vec<Event> + exit 0), mirroring the existing 'smoke-tested live against Anthropic and OpenAI' note in README. Pairs naturally with bl-5b5a (the bz-shim live-IO verification ball). Fix any fixture that diverges from the real wire.