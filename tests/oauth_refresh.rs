//! `OAuth2::apply` silent refresh (auth §6, §8): the only data-plane auth that
//! reads the clock and transport. Driven by `FakeClock` + `MockTransport` with an
//! in-memory store — fresh clock skips the refresh `POST`, stale clock refreshes
//! once (asserted body + persisted cred + new token on the wire), `invalid_grant`
//! and not-logged-in are 77, a transport drop is 69, and the auth-mode-dependent
//! `anthropic-beta` header rides along. No network, no real time.

use std::io;

use brazen::testing::{Chunk, FakeClock, MemoryCredStore, MockTransport};
use brazen::{
    Auth, AuthCtx, CanonicalError, Cred, CredStore, HeaderScheme, HeaderSpec, OAuth2Auth,
    OAuthConfig, ProviderCtx, RedirectSpec, Secret, Timeouts, WireRequest,
};
use serde_json::{Map, Value};

fn oauth_cfg() -> OAuthConfig {
    OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: None,
        client_id: "cid".into(),
        scope: None,
        beta_headers: vec![("anthropic-beta".into(), "oauth-2025-04-20".into())],
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
    }
}

fn oauth_cred(access: &str, refresh: &str, expires_at: u64) -> Cred {
    Cred::OAuth2 {
        access_token: Secret::new(access),
        refresh_token: Secret::new(refresh),
        expires_at,
        scope: Some("read".into()),
        account_id: None,
    }
}

/// Run `OAuth2Auth::apply` with the given store/clock/transport and an OAuth row,
/// returning the mutated wire.
fn apply(
    store: &dyn CredStore,
    now: u64,
    tx: &MockTransport,
    oauth: Option<&OAuthConfig>,
) -> Result<WireRequest, CanonicalError> {
    let header = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let beta: Vec<(&str, &str)> = Vec::new();
    let extra: Map<String, Value> = Map::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        api_header: &header,
        beta_headers: &beta,
        extra: &extra,
    };
    let authc = AuthCtx {
        store_key: "prov",
        inline_key: None,
        oauth,
    };
    let clock = FakeClock::new(now);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    OAuth2Auth.apply(&mut wire, &ctx, &authc, store, &clock, tx)?;
    Ok(wire)
}

#[test]
fn fresh_token_skips_refresh_and_sets_bearer_plus_beta_header() {
    let store = MemoryCredStore::with("prov", oauth_cred("at-fresh", "rt", 10_000));
    let tx = MockTransport::ok(vec![]);
    let cfg = oauth_cfg();
    let wire = apply(&store, 0, &tx, Some(&cfg)).unwrap();
    // No refresh POST: the transport was never sent.
    assert!(tx.requests().is_empty());
    assert_eq!(wire.header("Authorization"), Some("Bearer at-fresh"));
    // The auth-mode-dependent header (auth §4) is applied by this impl.
    assert_eq!(wire.header("anthropic-beta"), Some("oauth-2025-04-20"));
}

#[test]
fn account_header_is_emitted_from_the_cred_account_id() {
    // The auth-mode-dependent header whose VALUE is the cred's account id (auth
    // §10.4): NAME from the row, value from the stored cred. Fresh clock ⇒ no refresh.
    let cred = Cred::OAuth2 {
        access_token: Secret::new("at-fresh"),
        refresh_token: Secret::new("rt"),
        expires_at: 10_000,
        scope: None,
        account_id: Some("acct-1".into()),
    };
    let store = MemoryCredStore::with("prov", cred);
    let tx = MockTransport::ok(vec![]);
    let mut cfg = oauth_cfg();
    cfg.account_header = Some("ChatGPT-Account-ID".into());
    let wire = apply(&store, 0, &tx, Some(&cfg)).unwrap();
    assert_eq!(wire.header("ChatGPT-Account-ID"), Some("acct-1"));
    assert_eq!(wire.header("Authorization"), Some("Bearer at-fresh"));
    assert!(tx.requests().is_empty());
}

#[test]
fn stale_token_refreshes_once_persists_and_uses_the_new_token() {
    let store = MemoryCredStore::with("prov", oauth_cred("at-old", "rt-old", 100));
    // Refresh response OMITS refresh_token → prior is reused; new access token.
    let body = br#"{"access_token":"at-new","expires_in":3600}"#;
    let tx = MockTransport::ok(vec![body]);
    let cfg = oauth_cfg();
    let wire = apply(&store, 1_000, &tx, Some(&cfg)).unwrap();

    // Exactly one refresh POST, carrying the refresh grant + the prior token.
    let reqs = tx.requests();
    assert_eq!(reqs.len(), 1);
    let sent = String::from_utf8_lossy(&reqs[0].body);
    assert!(sent.contains("grant_type=refresh_token"));
    assert!(sent.contains("refresh_token=rt-old"));
    // Persist-then-use: the store now holds the new access token + reused refresh.
    match store.get("prov").unwrap() {
        Cred::OAuth2 {
            access_token,
            refresh_token,
            expires_at,
            ..
        } => {
            assert_eq!(access_token.expose(), "at-new");
            assert_eq!(refresh_token.expose(), "rt-old");
            assert_eq!(expires_at, 4_600); // absolute = now(1000) + 3600
        }
        _ => panic!("expected OAuth2"),
    }
    // The fresh access token is what goes on the wire.
    assert_eq!(wire.header("Authorization"), Some("Bearer at-new"));
}

#[test]
fn refresh_request_inherits_the_data_request_timeouts() {
    // The silent-refresh POST shares the hang risk of the data request, so it
    // carries the same bounds `run` stamped on the wire (config §4).
    let header = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let beta: Vec<(&str, &str)> = Vec::new();
    let extra: Map<String, Value> = Map::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        api_header: &header,
        beta_headers: &beta,
        extra: &extra,
    };
    let authc = AuthCtx {
        store_key: "prov",
        inline_key: None,
        oauth: Some(&oauth_cfg()),
    };
    let store = MemoryCredStore::with("prov", oauth_cred("at-old", "rt-old", 100));
    let tx = MockTransport::ok(vec![br#"{"access_token":"at-new","expires_in":3600}"#]);
    let clock = FakeClock::new(1_000);
    let bounds = Timeouts {
        connect: Some(5),
        response: Some(60),
        idle: Some(90),
    };
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    wire.timeouts = bounds;
    OAuth2Auth
        .apply(&mut wire, &ctx, &authc, &store, &clock, &tx)
        .unwrap();
    // The captured refresh POST carries the data request's bounds verbatim.
    assert_eq!(tx.requests()[0].timeouts, bounds);
}

#[test]
fn invalid_grant_refresh_is_auth_77() {
    let store = MemoryCredStore::with("prov", oauth_cred("at", "rt", 0));
    let tx = MockTransport::new(
        400,
        vec![Chunk::Data(br#"{"error":"invalid_grant"}"#.to_vec())],
    );
    let cfg = oauth_cfg();
    let err = apply(&store, 1_000, &tx, Some(&cfg)).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("bz login"));
}

#[test]
fn not_logged_in_is_auth_77() {
    let store = MemoryCredStore::new(); // no cred for "prov"
    let tx = MockTransport::ok(vec![]);
    let cfg = oauth_cfg();
    let err = apply(&store, 0, &tx, Some(&cfg)).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("bz login"));
    assert!(tx.requests().is_empty());
}

#[test]
fn missing_oauth_config_is_defensive_config_78() {
    // The resolve invariant (oauth is Some for an oauth2 row) — the defensive arm
    // is exercised directly with `oauth: None`, proving the no-panic contract → 78.
    let store = MemoryCredStore::with("prov", oauth_cred("at", "rt", 0));
    let tx = MockTransport::ok(vec![]);
    let err = apply(&store, 0, &tx, None).unwrap_err();
    assert_eq!(err.exit_code(), 78);
}

#[test]
fn transport_drop_on_refresh_is_69() {
    let store = MemoryCredStore::with("prov", oauth_cred("at", "rt", 0));
    let tx = MockTransport::new(200, vec![Chunk::Fail(io::ErrorKind::ConnectionReset)]);
    let cfg = oauth_cfg();
    let err = apply(&store, 1_000, &tx, Some(&cfg)).unwrap_err();
    assert_eq!(err.exit_code(), 69); // a transport read failure, not RefreshFailed
}

/// A store that holds an OAuth cred but fails every `put` — the refresh-persist
/// failure path.
struct FailPutStore;
impl CredStore for FailPutStore {
    fn get(&self, _: &str) -> Option<Cred> {
        Some(oauth_cred("at", "rt", 0))
    }
    fn put(&self, _: &str, _: &Cred) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}

#[test]
fn persist_failure_after_refresh_is_auth_77() {
    let tx = MockTransport::ok(vec![br#"{"access_token":"at-new","expires_in":10}"#]);
    let cfg = oauth_cfg();
    let err = apply(&FailPutStore, 1_000, &tx, Some(&cfg)).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("persist"));
}
