+++
title = "Encoder param-fidelity sweep: reasoning x temperature/top_p conflict on OpenAI chat+responses; Responses drops typed parallel_tool_calls; anthropic disable_parallel folded onto none/tool"
created = 1783466775
updated = 1783466775
priority = 17
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["openai", "spec-drift"]
+++
Arch-review finding (2026-07-07), three encoder-fidelity defects in one lane (all touch openai/responses encoders + specs):

1) reasoning x sampling conflict: openai/encode/mod.rs:40-48 and openai_responses/encode.rs:40-48 emit temperature/top_p unconditionally even when req.reasoning is set, alongside reasoning_effort/reasoning. Real OpenAI reasoning models (o-series/gpt-5) 400 on non-default temperature/top_p — the exact models that accept reasoning. The Anthropic encoder already omits them when reasoning is set (anthropic/encode/mod.rs:58-65, spec'd providers.md:435) — the asymmetry has no spec'd rationale. Live corroboration: the codex row had to strip max_output_tokens+temperature+top_p via unsupported_body_keys (bl-d54a) — a row-level workaround for what is really encode policy. Fix: when reasoning is Some, omit temperature/top_p in both OpenAI dialects (mirror the Anthropic rule); spec it in openai-chat-mapping.md + providers.md §3. Decide (spec-argued) whether the chat dialect should also emit max_completion_tokens instead of max_tokens when reasoning is set (openai-chat-mapping.md:141 currently defers the rename to row config, which 400s o-series when the row's body_defaults.max_tokens fills).

2) Responses encoder silently drops typed parallel_tool_calls (no reference in openai_responses/encode.rs; the wire SUPPORTS it). By the spec's own standard (providers.md CR-R1: a narrowing is OK only when the wire lacks the field — 'not a silent drop of a typed field that IS supported') this is a bug. Emit it top-level like chat (openai-chat-mapping.md:65). Also DOCUMENT the genuine empty-set drops in the Google and Ollama tables (neither wire has the knob; providers.md §4.2/§5.3 currently say nothing about parallel_tool_calls).

3) anthropic/encode/mod.rs:73-80 folds disable_parallel_tool_use:true onto EVERY non-empty tool_choice when parallel_tool_calls==Some(false), including {type:none} and {type:tool} where the field is undocumented/nonsensical. Restrict to auto/any; spec the edge in anthropic-messages.md §2.7.

All three: golden-fixture coverage, CHANGELOG, 100% coverage held.