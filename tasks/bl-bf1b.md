+++
title = "smoke: exercise prefix-routing (drop --provider for an unambiguous model)"
created = 1781727767
updated = 1781727767
priority = 2
tags = ["ergonomics"]
+++
bl-72dc (delivered, commit 9fee927; parent epic bl-ce84 now closed) made model->provider routing fire on model-id family prefixes (model_prefixes row data: anthropic ["claude-"], openai ["gpt-",...], google ["gemini-"], mistral ["mistral-",...]), so `bz --model claude-… "q"` now routes with NO --provider. scripts/smoke.sh does not cover this: every probe still passes --provider explicitly (line ~152 `args=(--provider "$provider" --model "$model" ...)`, plus the openai-chatgpt probes lines ~209-210).

Add a probe that runs an unambiguous model with --provider OMITTED and asserts it reaches the right backend (the bl-72dc end-to-end check: claude- -> anthropic, gpt- -> openai). This is the live proof of 'removes --provider from the one-liner' that bl-72dc's unit tests cover only at the resolve layer.

Scope/guards:
- Only providers whose default row ships model_prefixes can be probed provider-less: anthropic, openai, mistral, google. NOT openai-responses or ollama (ship no prefixes by design - they share/lack a stable family; see data/defaults.toml comments) and NOT the custom openai-chatgpt OAuth row.
- Keep skipping providers with no key present (smoke's existing per-provider skip).
- Reuse the existing probe helper; this is one added invocation per prefix-owning provider, not a new harness.