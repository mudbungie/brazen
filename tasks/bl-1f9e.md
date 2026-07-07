+++
title = "base_url injection without a config file: let an embedding harness point a run at a custom endpoint via flag/env"
created = 1783466961
updated = 1783466961
priority = 9
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["harness-support", "design"]
+++
Arch-review finding (2026-07-07). A harness cannot inject a provider endpoint without writing a TOML file: provider rows come only from data/defaults.toml or a user/--config file; the env layer (config/env.rs:31-62) and flag layer (cli/parse.rs:64-121) expose only scalar overrides (provider NAME, model, api_key, params, timeouts) — no base_url anywhere (grep: base_url exists only in defaults.toml + config parsing). So pointing bz at a local proxy, mock server, vLLM, or tenant-specific gateway requires generating a temp config.toml + --config per call — real friction for the flagship embedding case (lernie), for tests, and for multi-tenant gateways.

DESIGN QUESTION (spec-argue in config.md + architecture.md §5.10.3 before implementing; the CLI surface is a one-way door): the minimal severable shape is --base-url <url> / BRAZEN_BASE_URL as ONE more scalar in the existing fold (flag > env > file > row), overriding the RESOLVED row's base_url exactly like --model overrides model. It does not create a row — protocol/auth still come from the resolved row, which is the common case (same provider, different host). Weigh against: (a) the 'compiled config file you point to' stance (§6.2 — is a temp file actually fine and this flag mere sugar?); (b) scope creep toward full row injection (protocol/auth flags) — DECLINE that explicitly; a genuinely new provider is config-file territory. Precedent: every other row scalar the fold already lifts. If accepted: parse.rs, env.rs, resolve fold, config.md §4, architecture §5.10.3 table, tests incl. precedence row.