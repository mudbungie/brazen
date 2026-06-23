+++
title = "Fix Windows -D warnings build break: the cfg(unix)-gated concurrency test left Arc/AtomicBool/Ordering/thread imports unused on Windows -> unused_imports -> build fails (ci RUSTFLAGS=-D warnings). Scope them into the test."
created = 1782197398
updated = 1782197400
claimant = "Dialectic"
priority = 1
tags = ["bug"]
+++
Regression from bl-18d5. src/native/tests.rs:8-10 import Arc/AtomicBool/Ordering/thread, used ONLY by concurrent_reads_never_observe_a_partial_write (now #[cfg(unix)]). On Windows the test is gone but the imports remain -> unused -> -D warnings fails both windows jobs. Fix: move those three use lines into the test fn body so they gate with it. oauth_cred/store_at are used by other tests, so they stay. Close gate: make check green; reason about Windows (no local target) — only those 4 symbols are unix-exclusive (grep-confirmed).