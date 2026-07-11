+++
title = "Live-verify the 0.0.3 wave against real providers; replace synthetic goldens with recorded captures"
created = 1783491015
updated = 1783748213
claimant = "Lane-8f6a"
priority = 16
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["tests", "live"]
+++
Concern #1 from the 2026-07-08 arch-review close-out: ~15 lanes of new wire behavior shipped in 0.0.3 gated only by the offline gate, and several new 'golden' fixtures are SYNTHETIC (agent-authored from published wire references), diluting architecture.md §9.2's 'recorded from real streams, committed verbatim' guarantee. None of the following has touched a real provider:

- Reasoning round-trip (bl-61a9, THE priority): a full live thinking+tools agentic loop on Anthropic — decode signature via SignatureDelta, fold, re-encode, REPLAY, assert no 400 (the whole point; offline fixtures structurally cannot prove replay acceptance). Same for Responses store:false with include:[reasoning.encrypted_content] on the codex/ChatGPT-SSO row (see the bz-live-openai-chatgpt recipe), and Gemini thoughtSignature echo on a 2.5 function-calling turn if a Google key exists.
- bz --count-tokens (bl-24e5): live POST to Anthropic /v1/messages/count_tokens (and Google :countTokens if keyed); assert the N is sane vs the same request's live usage.input_tokens.
- Structured output (bl-0333): anthropic output_config.format GA acceptance (verify the wire spelling live — it was written from docs), openai response_format json_schema, ollama format (local keyless service, model bz-smoke:latest, per the ollama-local recipe).
- Content::Document (bl-956c): anthropic document block with a real small PDF base64.
- OpenAI mid-stream error + finish-without-[DONE] (bl-296d): recapture goldens from a real OpenAI-compatible endpoint where inducible (local ollama/vLLM-class), else document which fixtures remain synthetic and why.
- --raw=out (bl-8b56): capture a real encoded request wire and eyeball it against the specs' documented bodies — this flag exists precisely for this.
- --timeout (bl-f6ec): one live stall/quick-timeout sanity check (low value, cheap).

Credentials/recipes: the live memory recipes exist for Anthropic (Claude-Code OAuth bearer + system-prompt mandate), Codex/ChatGPT SSO row, and local Ollama (keyless). Precedents: scripts/smoke.sh, tests/live_fuzz_openai.rs / live_encode_openai.rs / live_support/, the #[ignore]d live conformance suite. Deliverable: run the passes, FIX what breaks (each fix its own honestly-scoped commit or spin-out ball if large), replace synthetic goldens with verbatim recorded captures where obtained, and record per-surface live status (verified / key-unavailable / synthetic-remains) in the close message. Do NOT commit secrets; captures must be scrubbed of tokens per the existing fixture discipline.