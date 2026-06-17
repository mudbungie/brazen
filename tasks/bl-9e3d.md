+++
title = "Decide: support explicit stream:false (drive() has no non-stream-2xx fold)"
created = 1781678224
updated = 1781679124
claimant = "Arousing"
priority = 3
tags = ["design"]
+++
After [bl-20d5] streaming is the implicit default, but an EXPLICIT `stream:false` (request/config) still breaks on SSE providers: `drive()` only frames a 2xx as a stream — there is no non-stream-2xx JSON fold, despite architecture.md §3.2 ('a non-stream provider response is the folded stream — the same Vec<Event>, produced in one decode call'). So §3.2 is currently unimplemented for 2xx.

Two options:
1. Implement the fold: drive() detects a single-JSON 2xx (e.g. when req didn't ask to stream, or by content-type/body shape) and runs decode once over the whole body (like the non-2xx whole_body path). Makes stream:false a real supported path + honors §3.2.
2. Decide brazen ALWAYS streams on the wire: drop stream:false support, document it, and remove/clarify the §3.2 'folded stream' claim for 2xx.

Pick one and align the spec. If (1): unblocks a stream:false smoke probe under bl-8cae.