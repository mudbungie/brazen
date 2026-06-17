+++
title = "Stop transport stall-test server thread from lingering ~10s after the test [bl-9940 follow-up]"
created = 1781669300
updated = 1781669761
claimant = "mark"
priority = 20
tags = ["impl"]
+++
serve() in bz/src/transport/tests.rs spawns a detached one-shot accept thread; stall_handler() writes one chunk then `thread::sleep(Duration::from_secs(10))` to hold the connection open past the idle timeout. After the client abandons the read (~1s) the test returns, but that server thread keeps sleeping in the background until it dies with the process.

Harmless today (Rust test harness does not join stray threads; they die at process exit), but it is a latent smell: ~10s of idle thread per stall test, and brittle if the binary is ever run under a harness that joins or under a thread-leak detector.

## Options
- (a) Have stall_handler hold the connection on a blocking read of the (already-abandoned) socket instead of a fixed sleep: once the client drops, the read returns/errors and the handler exits promptly — no fixed-duration sleep, no lingering thread. Cleanest (the connection close IS the signal).
- (b) Shorten the sleep to just over the test ceiling (e.g. matched to whatever the ceiling becomes in the sibling ball) — minimal but still a fixed lingering window.
- RECOMMENDED: (a) — dissolves the magic 10s constant; the handler lives exactly as long as the client.

Relates to the ceiling-vs-hang-guard ball (same file, same test).