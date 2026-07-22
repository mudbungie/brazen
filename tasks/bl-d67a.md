+++
title = "Config merge: a user [[provider]] list silently hides the embedded default rows"
created = 1784699259
updated = 1784699272
claimant = "Forborne"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
## Observed (2026-07-21, while shipping bl-b0b6)
The shipped claude-code default row (data/defaults.toml) was invisible on a machine whose ~/.config/brazen/config.toml declares its own [[provider]] rows: the user provider LIST replaces the embedded default list wholesale. bz --dump-config showed only codex+local; bz --provider claude-code failed to resolve until the row was hand-mirrored into the user file. That mirror is now two homes for one fact — the row will drift from defaults.toml on the next spec change.

## The design question
Is replace-semantics deliberate (explicit config: what you see in your file is all there is, no hidden rows) or an accident of list-valued TOML merge? Check specs/config.md for what is actually specified. If deliberate, the cost above should be stated in the spec and --dump-config might warn when defaults are shadowed. If accidental, consider merge-by-name: a user row with a matching name overrides that row; default rows with unmatched names persist; some explicit tombstone for removing a default.

## Deliverable
A decision recorded in specs/config.md (design first, per AGENTS.md), then the implementation or the documentation to match. Either way, the claude-code mirror in this machine's user config is the motivating test case.