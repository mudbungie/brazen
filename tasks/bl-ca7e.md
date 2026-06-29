+++
title = "Codex --list-models broken: recipe + user config omit [provider.models] (client_version query) — violates §10.5 'every Codex quirk handled by data'"
created = 1782719165
updated = 1782719165
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["bug"]
+++
## Symptom
`bz --list-models --provider codex` → HTTP 400 from the ChatGPT-SSO Codex backend:
`[{'type': 'missing', 'loc': ('query', 'client_version'), 'msg': 'Field required', 'input': None}]`

## Root cause
bl-eaf9 shipped the CAPABILITY (`[provider.models]` per-row discovery override: path/query/array_key/id_key) and its tests/specs, but never wired the codex RECIPE to use it. The codex `/models` route demands a `?client_version=X.Y.Z` query and returns `{"models":[{"slug":…}]}` (not the protocol-default `data[].id`). With no `[provider.models]` block the GET sends neither → 400.

This is config/recipe completeness, NOT a code bug: openai_responses serves BOTH standard OpenAI and codex, so client_version+slug are ROW data, never a protocol default. The override is the right (and only sound) home — bl-eaf9 built it for exactly this.

It directly violates auth.md §10.5 line 717: 'With it, **every** Codex quirk is handled by data, none by operator discipline.' The discovery quirk was the one omitted.

## Live probe (real backend, 2026-06-29)
client_version gating confirmed: 0.0.0 → full 7-model catalog; 0.1.0–0.99.0 → gated subset/empty (0.36.0→0, 0.99.0→3); >=1.0.0 → full 7. A list verb wants the FULL catalog, so pin the sub-release sentinel 0.0.0 (returns all 7; can't be retro-gated like a real version).

## Fix (config/doc only; capability already shipped)
1. auth.md §10.5: add [provider.models] (path=/models, query=[[client_version,0.0.0]], array_key=models, id_key=slug) to the recipe TOML + extend the 'every quirk by data' prose to cover discovery.
2. ~/.config/brazen/config.toml (user's live config, outside repo): same block on the codex row.
3. Rebuild + install bz; verify --list-models --provider codex lists the 7 models live.

Models returned: gpt-5.6-sol, gpt-5.6-terra, gpt-5.6-luna, gpt-5.5, gpt-5.4, gpt-5.4-mini, codex-auto-review.