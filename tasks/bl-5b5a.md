+++
title = "Verify bz-shim OAuth/cred IO: XdgCredStore atomicity test + live OAuth smoke test"
created = 1781650311
updated = 1781650311
priority = 45
tags = ["impl"]
+++
Two verification gaps left by bl-3c36 (OAuth2 + bz login), both in the coverage-excluded `src/bin/` shim so neither is exercised today.

## 1. XdgCredStore atomicity / mode (no test at all)
`src/bin/native.rs` writes creds via temp-file-create-0600 + fsync + rename, and chmods the dir 0700 (auth §5.2). None of this is tested. Add a `tempfile`-rooted test (own test crate or a `#[cfg(test)]` in a small testable shim) asserting:
- `get` after `put` round-trips a Cred::OAuth2;
- the written file mode is 0o600 on unix and the dir is 0o700;
- a concurrent reader never sees a partial write (rename atomicity).
Note: native.rs is currently coverage-excluded; either carve the pure-ish store logic into a lib-testable helper or add an integration test that drives the real store directly.

## 2. Live OAuth smoke test
All refresh/login tests use MockTransport — the real wire format is unverified against a provider. With the real ureq HttpTransport now landed (bl-838c), add a manual/opt-in smoke test (gated behind an env var / `#[ignore]`):
- `bz login <provider> --browser` and device flow against a real OAuth2 provider row;
- a data-plane run that triggers silent refresh and confirms the anthropic-beta header + 200.
Needs a real OAuth provider row (none ships for v0.1); coordinate with bl-2f13 (provider coverage).

## 3. (minor) Windows RNG
native.rs `fill_random` non-unix fallback is a weak time-seed, not cryptographic — fine per the documented Windows limitation (auth §5.2), but revisit (getrandom) if Windows becomes a target.