+++
title = "sever the preset catalog from the routing table: a row is routable only when named in config or holding a credential"
created = 1792725833
updated = 1792728804
claimant = "Gelded"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["feature"]
+++
## Ruling (operator, 2026-10-22)
The no-tombstone question dissolves: the real defect was ONE list wearing two hats. The merged provider list is the CATALOG (every preset defaults.toml ships + the operator's rows); the routing TABLE is the subset an act of the operator's has reached — a row NAMED in a config file (`[[provider]] name = "ollama"` suffices; the preset fills the body), or a preset a CREDENTIAL resolves for (stored / ambient / inline). Keyless presets enter only by name. By-name selectors (--provider, --login --provider, provider = ...) read the catalog.

## What landed
- `PartialProvider.preset` provenance bit: set only in `defaults()`, ANDed by the fold, serde(skip). `routable(name,row,inline,store)` (src/config/resolve/table.rs) is the one predicate; `route` scans the table on every tier + the zero-config head; `into_rows(keep)` feeds the listing. `into_resolved` takes `&dyn CredStore` beside the ModelCache.
- `--list-providers` prints the table. `bz --login` with no --provider prints the sign-in catalog (rows carrying an oauth block) in the same shapes, exit 0 — was NoProvider/78.
- Empty table → NoProvider (78) whose message names the acts (was 77 on anthropic).
- `auth::cred_state` shared by the listing column and the predicate.
- defaults.toml normalized: bearer/oauth2 rows drop the redundant `api_header` (row::complete supplies Authorization: Bearer); openai/openai-responses/mistral gain api_key_env ambient blocks (OPENAI_API_KEY / MISTRAL_API_KEY).
- antigravity NOT shipped as a preset: its recipe carries a client_secret brazen does not own (auth.md §11.2 note; rule: no preset whose credentials brazen does not own or whose vendor restricts third-party use).
- Specs: config.md §3.2 §6.1 §7, architecture.md §4.3.1 + listing bullet, auth.md §3.1 §5.5 §7 §11.2; README, SKILL.md.

## Consumers (evaluated; balls filed downstream)
lernie never runs bz — yog hosts brazen in-process and builds the `providers` reply; yog-android is the mobile seat on the same wire. Work: yog needs a catalog op (wrapping bare `bz --login`) and to pass through the table; lernie deletes the bl-526d filter, fixes `start::unsigned` for a worker whose row is absent from the table, and feeds the login pane from the catalog read; yog-android needs the parity surface.