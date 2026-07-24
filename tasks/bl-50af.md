+++
title = "Fix provider-row parsing under unlocked cargo install dependencies"
created = 1784863908
updated = 1784863908
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["config", "packaging"]
+++
## Reproduction

`cargo install --path . --force` on 2026-07-23 resolves serde/serde_derive 1.0.229 instead of Cargo.lock's 1.0.228. The installed `bz 0.0.3` then rejects a valid `[[provider]]` row with `data did not match any variant of untagged enum ProviderField`; the same source built with Cargo.lock parses it. Reinstalling with `cargo install --path . --force --locked` restores parsing.

## Deliverable

Make ordinary published/unlocked `cargo install brazen` parse provider rows reliably. Determine whether to pin serde 1.0.228 or remove the serde/toml interaction that regressed. Add a dependency-version regression check or focused fixture where practical; document any pin and its removal condition. All existing config remains compatible.