//! Live, opt-in OAuth smoke test (bl-5b5a, gap #2) — the ONE check that exercises
//! the real OAuth wire format against a real provider, which every other auth test
//! fakes with `MockTransport`. It is `#[ignore]`d (so it never runs in CI or the
//! coverage gate) AND env-gated (it no-ops with a printed reason unless an operator
//! supplies a real OAuth2 provider row), because:
//!
//!   * no `oauth2` provider row ships in `data/defaults.toml` for v0.1 — that is
//!     blocked on bl-2f13 (provider coverage), so the operator points `bz` at a
//!     config carrying one;
//!   * step 1 is interactive (a browser opens for consent, RFC 8252 AuthCode +
//!     loopback) — there is nothing to automate.
//!
//! Run it by hand once you have an oauth2 row + an account:
//!
//! ```text
//! # a config file (TOML) with a [[provider]] whose `auth = "oauth2"` + `[oauth]`
//! # block; `bz login`/`bz run` both read it via $BRAZEN_CONFIG (login takes no
//! # --config flag — config-file resolution is $BRAZEN_CONFIG > XDG).
//! export BRAZEN_CONFIG=/path/to/oauth.toml
//! export BZ_SMOKE_PROVIDER=claude            # the provider/row name in that file
//! cargo test -p brazen --test oauth_smoke -- --ignored --nocapture
//! ```
//!
//! Step 2 issues a real data-plane request. If the stored access token has aged past
//! expiry it drives the silent refresh (auth §6) and the `anthropic-beta` header;
//! either way an exit of 0 means the bearer + beta header were accepted end-to-end
//! and the provider answered `200`. To eyeball the actual outbound headers, point
//! the operator's config `base_url` at a logging proxy.

use std::process::{Command, Stdio};

/// The provider/row name to log in to and run against; `None` (env unset) → skip.
/// `$BRAZEN_CONFIG` is read by `bz` itself from the inherited environment.
fn smoke_provider() -> Option<String> {
    std::env::var("BZ_SMOKE_PROVIDER")
        .ok()
        .filter(|p| !p.is_empty())
}

#[test]
#[ignore = "live OAuth: needs a real oauth2 row + interactive browser; run with --ignored"]
fn browser_login_then_refreshing_data_plane_run() {
    let Some(provider) = smoke_provider() else {
        eprintln!(
            "skipping live OAuth smoke: set BZ_SMOKE_PROVIDER (+ $BRAZEN_CONFIG pointing \
             at a config with that oauth2 row) to run it"
        );
        return;
    };
    let bz = env!("CARGO_BIN_EXE_bz");

    // Step 1 — interactive browser login (RFC 8252 AuthCode + loopback). Blocks on
    // the operator completing consent in the browser, then persists a Cred::OAuth2.
    let login = Command::new(bz)
        .args(["login", &provider, "--browser"])
        .status()
        .expect("spawn `bz login`");
    assert!(
        login.success(),
        "`bz login {provider} --browser` failed: {login:?}"
    );

    // Step 2 — a data-plane run selecting that provider row. With a stale access
    // token this exercises silent refresh + the `anthropic-beta` header; with a fresh
    // one it still proves the bearer + beta header are accepted. Exit 0 == HTTP 200.
    let run = Command::new(bz)
        .args(["--provider", &provider, "reply with the single word: ok"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("spawn `bz run`");
    assert!(
        run.success(),
        "data-plane run for `{provider}` failed: {run:?}"
    );
}
