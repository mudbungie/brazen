+++
title = "Assemble run() spine + os/browser.rs + main shim + SIGPIPE"
created = 1781559069
updated = 1781559088
priority = 62
tags = ["impl"]

[[blockers]]
id = "bl-2135"
on = "claim"
+++
Wire run(args, stdin, stdout, transport, store, clock) end-to-end: resolve -> parse -> registry lookup -> encode (or raw identity) -> auth.apply -> transport.send -> status-driven exit -> framing decode -> sink -> End. Implement os/browser.rs (browser_argv tested as data for all three OS). Write src/bin/main.rs (the ~5-line shim wiring HttpTransport/XdgCredStore/SystemClock, restore_sigpipe, call run; #[coverage(off)]). Full-run integration tests with MockTransport across every mode incl. raw-4xx-exit-69, refusal-exit-0, transport-drop, BrokenPipe->141. v0.1 vertical slice complete here.