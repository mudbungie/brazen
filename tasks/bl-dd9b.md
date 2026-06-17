+++
title = "Tool-result name alignment: Google functionResponse.name emits the synthesized id, not the function name (illegal call)"
created = 1781661296
updated = 1781661380
claimant = "Motored"
priority = 55
tags = ["bug"]
+++
Discovered during bl-aba5 (spec-validation of the four providers; no live test had exercised the tool round-trip).

## The bug (must-fix)
Google keys a tool RESULT to its CALL by **function name** (`functionResponse.name`), not by id. Our encoder (`src/protocol/google_genai/encode.rs`, `fn function_response`) emits the canonical `tool_use_id` as that name.

Round-trip: on DECODE Google sends a `functionCall` with a name but **no id**, so brazen synthesizes one (`call_{candidateIndex}_{block_index}`). The harness replays the transcript and sends back `ToolResult{tool_use_id:"call_0_0"}`; the encoder emits `functionResponse.name:"call_0_0"` where Google expects `"get_weather"`. **The model cannot associate the response with the call — tool-use through Google is silently broken.** The 3 id-keyed dialects (Anthropic / openai_chat / openai_responses) are correct; only the two NAME-keyed dialects are affected.

## Elegant fix — one invariant, two consumers
The function name is a fact that lives **once**, on the assistant `Content::ToolUse{id, name}`. The `ToolResult` references it by `tool_use_id`. brazen is stateless, but the harness replays the FULL transcript, so the originating `ToolUse` rides in the **same request** as the result — the name is resolvable in-request, no state required.

Add one shared query on the canonical request (make it a query, NOT a field on `ToolResult` — a copied name denormalizes and drifts; SSOT):

    CanonicalRequest::tool_name(&self, tool_use_id) -> Option<&str>   // scans Content::ToolUse across messages

Consumers:
- **Google** `function_response`: use `tool_name(id).unwrap_or(id)` as `functionResponse.name`.
- **Ollama** `tool_messages`: emit `tool_name` when `Some` (Ollama /api/chat tool messages carry `tool_name`; we omit it today — works positionally but is not aligned). Omit when `None`.

## Edge (stays clean, no fabrication)
If the referencing `ToolUse` is absent from the request (harness sends a bare tool-result turn), the name genuinely is not in-band → Google falls back to the id, Ollama omits `tool_name`. The legitimate "fact absent" case.

## Spec correction (do not assume the doc is right)
`specs/providers.md` §4.5 currently states `ToolResult.tool_use_id is projected back to functionResponse.name` and CR-G1 records it as "no architecture change required." **That conclusion is wrong — it produces an illegal call.** Amend §4.5 to specify name-resolution, reframe/close CR-G1, and update the §5 Ollama tool-message row to include the resolved `tool_name`.

## Tests / acceptance
- Golden encode fixtures: a transcript with a `ToolUse` then a `ToolResult` asserts `functionResponse.name` == the function name (Google) and `tool_name` present (Ollama); plus the absent-ToolUse fallback. Pure encode path → 100% line coverage holds.
- `make smoke` (with a real key) shows a clean multi-turn Google tool call.