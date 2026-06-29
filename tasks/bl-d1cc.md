+++
title = "Data plane learns the model on success (zero-config bz 'just works')"
created = 1782719470
updated = 1782719470
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
A 2xx generation with a VERBATIM model (one the cache could not place) appends it to that provider's model cache, so a later bare 'bz' defaults to it.

Fixes zero-config 'bz yo' for providers whose --list-models is broken/unrun (e.g. codex): a single successful '--provider codex --model gpt-… yo' seeds the cache; next 'bz yo' resolves the empty seed to it.

Data plane becomes a SECOND cache writer (sibling of OAuth refresh's cred write on the data plane); list-models stays the authoritative wholesale writer. A Cached model is already in the list, so only verbatim-success writes — no churn, fully covered.

Reconcile specs: model-discovery §5 / architecture §6.5 'read-only on the data plane' / config.md §116 sole-writer claims.