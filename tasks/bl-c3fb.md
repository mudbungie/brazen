+++
title = "Clarify/rename synthetic determinism fixtures sse_anthropic.sse / sse_openai.sse — toy-grammar, not real provider wire (misleading names)"
created = 1782602936
updated = 1782602936
priority = 6
tags = ["impl"]
+++
Spun out of bl-2fd5 (corrects an over-stated concern from that review). `tests/fixtures/sse_anthropic.sse` and `sse_openai.sse` are fed ONLY to the self-contained rechunk-determinism test `src/tests/protocol_sse_determinism.rs`, which uses its OWN toy decoder over a shared mini JSON grammar (`usage.input`/`usage.output` -> canonical input_tokens/output_tokens). They are NOT real provider captures and are NOT fed to the real Anthropic/OpenAI `Protocol::decode`. The test is correct and meaningful (it asserts rechunk-invariance + the universal event invariants).

The issue is purely clarity: a fixture named `sse_anthropic.sse` containing `{"type":"message_delta","usage":{"input":12,"output":34}}` strongly implies a real Anthropic capture, when real Anthropic wire uses `input_tokens`/`output_tokens` (and a different message_delta shape). A future contributor could mistake the toy grammar for the wire contract.

Fix (NOT a functional bug): rename the fixtures to something that reads as synthetic (e.g. `determinism_sse_a.sse`/`_b.sse`) and/or add a doc comment in the test stating they are a shared toy grammar, deliberately divergent from any real provider dialect. Do NOT switch them to real provider keys — the test uses ONE mini-parser across anthropic/openai/ollama, so provider-specific key names would break that design. Low priority.