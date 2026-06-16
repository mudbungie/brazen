+++
title = "Assemble run() spine + os/browser.rs + main shim + SIGPIPE"
created = 1781559069
updated = 1781639362
claimant = "Niggles"
priority = 62
tags = ["impl"]

[[blockers]]
id = "bl-2135"
on = "claim"
+++
Wire run(args, stdin, stdout, transport, store, clock) end-to-end: resolve -> read input (read_request: positional-prompt argv XOR stdin canonical request; both present -> exit 64) -> parse -> registry lookup -> encode (or raw identity) -> auth.apply -> transport.send -> status-driven exit -> framing decode -> sink -> End. read_request lives in pipeline/input.rs (open_input + parse + the sinks/pump already landed in bl-faf3); it builds CanonicalRequest{messages:[User Text(prompt)]} from argv, reads stdin only when no positional prompt, and config/flags fill system/model/gen-params via fill_absent. Implement os/browser.rs (browser_argv tested as data for all three OS). Write src/bin/main.rs (the ~5-line shim wiring HttpTransport/XdgCredStore/SystemClock, restore_sigpipe, call run; #[coverage(off)]). Full-run integration tests with MockTransport across every mode incl. positional-prompt==stdin parity, positional+stdin->64, raw-4xx-exit-69, refusal-exit-0, transport-drop, BrokenPipe->141. v0.1 vertical slice complete here.