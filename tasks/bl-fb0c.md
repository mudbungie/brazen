+++
title = "report the model's context window in-band: an additive context_window field on the Usage event, so a harness that makes no model-list call can divide the counters it already records by the window they fill"
created = 1788493096
updated = 1788493096
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Sibling of bl-75f7 (the read surface). litany keeps no per-model table and performs no model-list call (its ARCH 4.2, bl-35e2); it records each Usage event's counters beside the model output it belongs to and reads them back from that entry. A usage-based compaction trigger (litany docs/DESIGN_CONTEXT_ECONOMY.md section 5.1) needs the denominator on the same event: an additive, Option-shaped context_window on Usage — the same fact bl-75f7 lifts onto list-models (provider-reported where served, config-declared on the row otherwise), carried in-band on every stream, absent when unknown, never fabricated. v=1 additive per specs/architecture.md (the Usage struct is non_exhaustive; a new counter is additive). Verify the actual shape before choosing: whether the fact rides Usage or MessageStart is brazen's decision; the consumer constraint is only that it arrives on the event stream beside the counters and is recorded, never computed. Test: the anthropic/openai/google encoders' fixtures carry the field when the row states it and omit it otherwise.