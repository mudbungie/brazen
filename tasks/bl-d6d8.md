+++
title = "ErrorKind wire schema: add a tolerant Other catch-all before any 0.1.0 freeze (owner-decided) + reconcile #[non_exhaustive]/§3.3/§13.9"
created = 1782688496
updated = 1782689559
claimant = "mark"
priority = 3
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
OWNER DECISION (Q3): ADD a tolerant Other so a future 8th error kind degrades gracefully on a 0.1.0-pinned consumer (a shipped binary cannot be made tolerant retroactively). ErrorKind (src/canonical/error.rs:24-35) is the ONLY wire enum with no Other catch-all, yet the error event carries no v handshake (§13.9 'frozen, not version-gated'). It is also self-contradictory: #[non_exhaustive] (signals growth) vs §13.9 (frozen).

Fix:
- Add ErrorKind::Other (carrying the unknown wire tag verbatim, mirroring Event::Other/ContentKind::Other/Delta::Other) with a ~15-line hand-rolled tolerant Deserialize routing any unknown snake_case kind -> Other (Serialize must round-trip it back). Keep from_http_status / exit / retryable total over the new variant (Other -> a sensible class, likely Software/70 or Transport/69 — decide + test).
- Amend architecture.md §13.9 to PERMIT additive growth within the frozen error schema (Other is the escape hatch), and reconcile the §3.3 block (add the #[non_exhaustive] attr it omits) so all three (code, §3.3, §13.9) agree.

** COORDINATION: this edits src/canonical/error.rs, which is in the ACTIVE library-vocabulary lane (Snaring, bl-b4a9 're-expose the canonical vocabulary'). DO NOT start until bl-b4a9 is closed (check 'bl list -s claimed'); then claim, rebase on main, implement. Not a 0.0.x cut-blocker (doors open per Q1) — this is runway to 0.1.0. **

SCOPE when unblocked: src/canonical/error.rs + its tests (src/tests/canonical_error.rs) + architecture.md §3.3/§13.9. make check gate; the new Other arm + tolerant-decode path must be covered (add an unknown-kind fixture, mirroring tests/canonical_event.rs).