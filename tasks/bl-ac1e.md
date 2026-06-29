+++
title = "Zero-config default must be the user's FIRST-DECLARED provider, not first-by-name (built-in defaults shadow user rows)"
created = 1782712269
updated = 1782712269
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
## Bug (regression from bl-b911)
`bz yo` with a user config that declares `[[provider]] name="chatgpt"` (then `local`)
routes to **anthropic**, a built-in `data/defaults.toml` row the user never configured.

## Root cause
`PartialConfig.providers` is a `BTreeMap<String,_>` keyed by name; the built-in default
rows are folded into the SAME table as the user's rows. bl-b911 picked the no-model
default as `providers.iter().next()` = first by NAME = `anthropic`, shadowing the user's
`chatgpt`. Also: the `provider` TOML key is overloaded (string=selector, `[[provider]]`=rows,
same key in partial_de.rs), so a rows-config CANNOT also set a `provider="x"` selector — the
user has no way to name a default except the row order.

## Fix
The zero-config default = the FIRST provider DECLARED (config-file order), built-ins ranked
below user rows. Carry the fact the BTreeMap discards (AGENTS.md "carry the fact, don't
reconstruct from a lossy proxy"):
- partial.rs: add internal `default_provider: Option<String>` to PartialConfig + fold it in `or`.
- partial_de.rs: in the Rows arm, set `default_provider = rows.first().name`. The fold's `.or()`
  makes the user file's first-declared outrank defaults' first-declared (anthropic), so a user
  config's first row wins. flags/env never add rows, so the invariant "providers non-empty =>
  default_provider Some" holds.
- resolve.rs route(): no-model branch picks `default_provider` (looked up in the map), else
  NoProvider. This is DISTINCT from the `provider` selector (which is checked first and overrides
  model routing); the default is only the no-model/no-selector fallback, so prefix routing
  (`bz -m gpt-5`) is untouched.
- login unchanged: `resolve_oauth` still guards on the `provider` SELECTOR, so `bz --login`
  with rows-only config stays NoProvider (a credential write names its target).

## Tests
- config_route zeta-before-alpha test: assert first-DECLARED (`zeta`), not alphabetical (`alpha`).
- NEW: user file [chatgpt, local] merged with REAL defaults() + no model/provider -> `chatgpt`
  (the exact regression: defaults must not shadow).
- e2e tests using defaults-only config still route to anthropic (defaults' first-declared = anthropic).

## Specs
arch §4.3, config §7 routing step 2 + NoProvider row, model-discovery §2, README, errors.rs +
resolve.rs route() doc, auth §7 (login still needs the selector): "first by name" -> "first declared".