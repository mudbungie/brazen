+++
title = "reasoning: max_tokens coupling on the Google wire — probe Gemini, then one shared floor + fill-from-served-max_output"
created = 1790733060
updated = 1790733060
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Design proposal, 2026-09-29, resolves bl-0bc9 by principle rather than a per-model patch. Today only the Anthropic encoder floors max_tokens at budget()+4096 (providers.md §6); the Google encoder emits thinkingBudget with no coupling, and the Cloud Code backend fronting Vertex-hosted Anthropic 400s ('max_tokens must be greater than thinking.budget_tokens') because its own default max_tokens is small.

Principle (explicit vs ergonomic): a value the caller STATED is never silently lowered, only raised to what the provider can accept (the existing bump-don't-error rule); a value the caller did NOT state is filled only from a fact brazen actually holds, never invented.

Step 1, probe (like bl-739a): on antigravity, gemini-2.5-flash with --reasoning medium and --max-tokens 1100: does Gemini 400, truncate, or ignore? That decides whether the floor is Anthropic-only data or a property of every budget dialect. Also record what the Cloud Code listing serves as maxOutputTokens for claude-sonnet-4-6 / claude-opus-4-6-thinking.

Step 2, design (adjust to the probe):
 (a) the floor max(max_tokens, budget+headroom) moves from the Anthropic encoder to the ONE canonical normalization site (config §4.1.1, beside fill_absent/strip_unsupported) and applies whenever a budget dialect will carry the budget and max_tokens is Some; encoders just emit req.max_tokens.
 (b) absent max_tokens + reasoning set: fill from the resolved model's served max_output_tokens (cache, §3; the same local read §5.5 already performs for the window), else leave absent. Gemini is unchanged (its served max IS its default); Claude-via-Cloud-Code gets an honest explicit cap above any budget. No new flag, no per-model branch.
 Interim zero-code workaround: the antigravity row can pin body_defaults = { generationConfig = { maxOutputTokens = N } } (one-level merge; typed max_tokens wins) — an operator guess, so not the design.

Spec homes: providers.md §6 (the coupling paragraph), config.md §4.1.1, model-discovery.md §5 if the cache read is reused; bl-0bc9 closes as a duplicate when this lands.