+++
title = "Pre-0.1.0 one-way-door review: timeout knob taxonomy — three flags (connect/response/idle) or one 'upstream is not sending' budget?"
created = 1783472192
updated = 1783472192
priority = 6
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design"]
+++
Owner musing 2026-07-08, filed for pre-freeze review because the three flags (--timeout-connect/--timeout-response/--timeout-idle) are part of the §5.10.3 surface that FREEZES at 0.1.0 — collapsing them later is a breaking change; keeping three where one suffices is a permanent shackle ('each knob is a shackle of forward compatibility').

The owner's words: a wall-clock total timeout is REJECTED (skip it — 'you don't care why, you timed out' is true but it's a footgun and the upstream has the tools: kill the child). And: 'I'm not even sure I like the connect/response timeout bifurcation. Upstream isn't talking is one thing, but ultimately there's not a need for a L7 protocol to be tracking L4 TCP SYNACKs; if it's not sending, it's not sending.'

The review question: are connect (no SYNACK), response (no header bytes), and idle (no body bytes between chunks) three facts or ONE fact — 'silence longer than N'? If one: a single --timeout-idle (or --timeout-silence) spanning connect-through-last-byte, with the other two flags never shipping in 0.1.0. Feasibility gates the answer: check what ureq 3's timeout API actually distinguishes (connect timeout may be Agent-config-level; the idle mechanism is brazen's own IdleChunkReader thread — could it own the whole silence budget from send() onward?). Also weigh the one real asymmetry: a connect that will never succeed fails fast with a distinct OS error vs a server that accepted and stalls — does the caller ever need different budgets for those?

Deliverable: the argument + recommendation as a spec decision note (architecture.md §5.10.3 + §13), then implementation if the collapse wins. NOT auto-dispatched — the ruling is the owner's; this ball holds the question so the freeze does not ship it unexamined.