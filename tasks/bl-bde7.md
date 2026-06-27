+++
title = "Config schema forward-evolution invariant: a config valid today must stay valid in all future versions (additive-only)"
created = 1782588032
updated = 1782589680
claimant = "Pincushion"
priority = 3
tags = ["interface-review", "design"]
+++
## Context

One-way-door review (2026-06-27). The on-disk config (TOML: `PartialConfig` + provider
rows) has no version field. Per config.md, rows are `deny_unknown_fields` and the top level
has a `#[serde(flatten)] extra` valve.

## Decision (owner)

Config needs only **forward evolution compatibility**, not backward: a config file valid
today must remain valid (same meaning) in ALL future brazen versions. We do NOT need an
older brazen to read a config authored for newer features (no downgrade support). The other
on-disk formats are safe as-is — the model cache self-heals (regenerable by `list-models`),
and credentials are already locked (absolute `expires_at`, variant-as-discriminant).

## Deliverable

Record the invariant in config.md (living doc) and enforce by convention:
- never rename, remove, or repurpose an existing config key, or change its meaning;
- evolution is additive-only: new keys are optional with serde defaults;
- `--dump-config` output pins brazen's defaults-at-dump-time (it omits the defaults layer
  and carries no version marker) — document that a dumped config freezes those defaults
  forever for that file.

No version field, no migration machinery — rejected as not worth the complexity; the
additive-only discipline + tolerant readers suffice.

Type: **DESIGN** (records an invariant; small).