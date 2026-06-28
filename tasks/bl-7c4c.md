+++
title = "OAuth token expiry: saturating_add for now+expires_in (panic-on-external-input, §8); add huge-expires_in fixture"
created = 1782681342
updated = 1782681342
priority = 2
+++
Confirmed: provider-controlled expires_in (u64 seconds) near u64::MAX overflows now+secs. Debug/test builds PANIC (violates architecture.md §8 'no panic on external input' and §9.5 'no panic on the data path'); release builds wrap to an immediately-stale instant. Pure internal fix, touches no frozen interface.

Sites:
- src/auth/oauth.rs:131-134 — expires_at = match raw.expires_in { Some(secs) => now + secs, None => jwt_exp(&access).unwrap_or(now) }. Change now + secs -> now.saturating_add(secs).
- src/auth/flows.rs:37 — the analogous now + expires_in in the device/login flow. Same saturating_add fix.

Wrapping/saturating to a far-future-or-stale instant is the correct degradation: a bogus huge expires_in just means the token is treated as long-lived (saturated) rather than panicking; the next real 401 forces a refresh regardless. Confirm there is no other now+<provider value> on the auth path.

CONSTRAINTS: make check close gate = 100% line coverage + clippy -D + fmt + 300-line cap. Add a test feeding a near-u64::MAX expires_in through the token parse (src/tests/oauth_*.rs / src/tests/oauth_refresh.rs) asserting saturating behavior and NO panic, so the new saturating path is covered.