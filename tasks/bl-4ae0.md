+++
title = "Doc reconciliation bundle: fix single-source-of-truth drift in architecture.md + auth.md to match shipped code (review findings)"
created = 1782681477
updated = 1782698770
claimant = "mark"
priority = 4
+++
Confirmed doc-only drift from the pre-release review (code is correct; the SPECS are stale). Bundle, do as one pass POST-cut (architecture.md is also being edited by the active CLI lane — sequence to avoid conflict on §5.10/§13):
- architecture.md §3.2: redacted-thinking data does NOT ride content open/close (code/dependent-spec contradict the master).
- §3.1/§5.3: scope the word 'lossless' to EXCLUDE the deferred Anthropic thinking signatures (v0.1 cannot round-trip them).
- §4.1: Protocol has 8 methods; AuthCtx carries the ambient field — update the trait listing.
- §4.5: beta_headers are DATA, not a literal.
- §5.1/§5.8 + main.rs:~136 comment: the production exit/SIGPIPE mechanism is respond::write_event/from_io, NOT the test-only pump.
- §6.4: the frozen cred format includes account_id + serde defaults; the config path is XDG on all platforms (not the macOS/Windows variants the master implies).
- §9.5: the cov ignore regex is src/(main.rs|native|tests).
- auth.md: §5.5 ambient is operator-supplied (not 'zero-setup'); §6.2 (see the refresh task); §7.2/§10.1 CodeReceiver::bind (not 'port'); line ~542 stale 'bz/' regex.
- src/testing/mod.rs header: the doubles are #[cfg(test)] private, not shared with the bin.
No code change; pure doc. AGENTS.md: 'don't implement a deviation: fix the doc.'