+++
title = "Model discovery: list-models verb + default/partial model resolution via lazy live probe"
created = 1781802412
updated = 1781802412
priority = 50
+++
Spec the capability so bz 'just works' on imprecise model input.

Decision (user, Twiggy session): LAZY LIVE PROBE. A fully-specified model stays one round-trip; an imprecise model (partial like 'opus', or absent -> needs a default) prepends ONE model-list GET to the resolved provider, then the generation round-trip. No cache (honors no-state non-goal); no fan-out (probe hits the single resolved provider).

Three behaviors:
1. `bz list-models [--provider X]` control verb (sibling of `bz login`): one round-trip GET to the provider's models endpoint, auth via the same seam, prints ordered ids (+ default), --json available.
2. Default selection when model absent: provider's suggested default if the API/row marks one, else first in list order.
3. Partial matching: a model that matches none of the resolved provider's model_prefixes (and is no exact alias) is shorthand -> expand against the live list: exact id wins, else first id containing the partial in list order ('the suggested version').

Deliverable: specs/model-discovery.md (new, derives from architecture) + CR amendments to architecture.md (§1 round-trip, §4.3 resolution), providers.md (per-provider models endpoints), config.md (resolution flow), README. Implementation follows as separate tasks.