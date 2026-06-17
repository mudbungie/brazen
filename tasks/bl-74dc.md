+++
title = "Per-row body defaults: generalize default_max_tokens into a body_defaults map so rows can pin store/stream/etc (config + auth §10.5)"
created = 1781675003
updated = 1781675849
claimant = "Scuffles"
tags = ["design"]
+++
Add per-provider-row body defaults so a `[[provider]]` row can pin request-body fields the backend always needs, instead of the user hand-crafting canonical JSON every call. Design-first (amend the spec before code).

## Motivation
The OpenAI ChatGPT-SSO codex backend (auth §10) mandates `store: false` and `stream: true` on every request; today the only way to set `store` is a hand-written canonical request with a flattened `extra`. A row default makes the ergonomic path:

```toml
[provider.body_defaults]
store  = false
stream = true
```
→ `bz --provider openai-chatgpt --model gpt-5.4 --system "…" "hi"` just works.

## This is a GENERALIZATION, not a bolt-on (single source of truth)
`default_max_tokens` is ALREADY a per-row body default (it sets `max_output_tokens`/`max_tokens`). The clean move is to fold it into ONE mechanism — a `body_defaults` map — so `default_max_tokens` becomes `body_defaults.max_output_tokens` rather than a second, parallel concept. Decide explicitly: generalize-and-migrate vs keep both (the former is the SSOT-correct answer; check all call sites + defaults.toml + config tests).

## Design questions to settle in the spec
- **Precedence.** Explicit request (typed field / `extra`) and flags must beat row `body_defaults`; row default beats nothing. Mirror the existing `fill_absent` (config gen-param fill) and the encode `body.entry(k).or_insert(v)` "typed fields win" rule. Define the full order: flag > request.extra/typed > row body_defaults > protocol baseline.
- **Seam.** `ProviderCtx` already carries an `extra: &Map` handed to `encode` — check whether it is wired from config and could BE this, vs adding a dedicated field. Prefer reusing an existing seam over a new one (deep/narrow interface).
- **Severability / not a junk-drawer.** It must not let a row override fields the canonical model owns in a way that desyncs the canonical→wire mapping; document the boundary. Removing the block must delete config, not edit code.
- **Redaction / dump-config.** `--dump-config` must show body_defaults (no secrets expected, but keep the redaction discipline).

## Deliverable
Spec amendment (config.md + a note in auth §10.5 pointing at the OpenAI use case), then implementation with the `default_max_tokens` migration, tests at 100% line coverage, README update. Unblocks the clean `openai-chatgpt` invocation; relates to bl-04dc / bl-b72f (live tests would use it).