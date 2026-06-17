+++
title = "Live-plumb the openai_responses ENCODE circuits unexercised by bl-b72f: image content, tool_choice required/none spellings, full tool-result round-trip"
created = 1781680531
updated = 1781681590
claimant = "Push"
priority = 45
tags = ["testing", "openai"]
+++
Follow-up to bl-b72f (OpenAI ChatGPT-SSO fuzz). The fuzz harness exercised text + a tool DEFINITION/CALL, but several brazen encode circuits in src/protocol/openai_responses/encode.rs are still unvalidated against the live Codex backend. Each is a candidate silent encode mismatch (the point: plumb brazen, not OpenAI).

## Circuits to drive live (`--provider openai-chatgpt`, model gpt-5.4, store:false/stream:true/instructions, NO temperature/top_p/max_tokens per bl-d54a)
1. IMAGE content -> `input_image` (encode::input_image): base64 -> `data:<mt>;base64,<data>` data-URI, and a URL passthrough. Assert the codex backend accepts brazen's `input_image` shape (correct field name `image_url`, data-URI format) and the request 200s. A vision-capable model may be required; if the codex row's model rejects images, that itself is a finding to record.
2. tool_choice SPELLINGS (encode::tool_choice_value): `Any`->`"required"`, `None`->`"none"` (the fuzz only drove `Tool{name}`->`{type:function,name}`). Assert codex accepts `"required"` and `"none"` (it may expect a different shape). Each is one request; `"none"` should yield NO tool call, `"required"` should force one.
3. Full tool-RESULT round-trip (encode::function_call_output + message_items hoisting): a multi-turn request carrying a prior assistant `tool_use` AND a `tool`-role `tool_result` -> brazen emits `function_call` + `function_call_output` items keyed by call_id. Assert codex accepts the fed-back result and continues. The fuzz never sent a tool RESULT, only a tool def. `is_error:true` textual prefix is part of this circuit.

## Constraints
Opt-in/#[ignore]d + BRAZEN_LIVE-gated, built on bz/tests/live_support/ leaves (reuse, do not duplicate). These GENERATE -> gate token-costing ones behind BRAZEN_LIVE_FUZZ_SPEND like live_fuzz_openai.rs. Bound volume + LOG skips/caps (AGENTS.md). Any mismatch found becomes its own ball.

## Watch out
Transient non-2xx under rapid repeated live calls (exit 69) — re-run a lone FAIL before treating it as a regression (recorded in the bz-live-openai-chatgpt-testing note).