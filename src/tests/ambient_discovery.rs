//! Ambient credential discovery at the auth layer (auth §5.5): a resolved provider
//! row that names an `ambient` source pulls its credential on a store miss, so a run
//! needs no `--api-key` and no `bz login` when a foreign tool (Claude Code) is signed
//! in. `MemoryCredStore::with_ambient` models that file with no disk — the real file
//! read + `$HOME` expansion are the `bz` shim's, pinned in `bz/src/native/tests.rs`.
//! The shared `fetch_cred` (`store.get → discover`) is exercised through both the
//! `StaticSecretAuth` and `OAuth2Auth` impls — one credential-source decision, two
//! consumers.

use crate::testing::{FakeClock, MemoryCredStore, MockTransport};
use crate::{
    AmbientFormat, AmbientSpec, Auth, AuthCtx, CanonicalError, Cred, CredStore, HeaderScheme,
    HeaderSpec, OAuth2Auth, OAuthConfig, PartialConfig, RedirectSpec, Secret, StaticSecretAuth,
    WireRequest,
};

/// A Claude-Code-style ambient source (auth §5.5).
fn ambient_spec() -> AmbientSpec {
    AmbientSpec {
        format: AmbientFormat::ClaudeCode,
        path: "~/.claude/.credentials.json".into(),
    }
}

/// Apply `auth_impl` against a row that names an ambient source, returning the wire.
/// The header is `Authorization: Bearer` (shared by both impls under test); `oauth`
/// is supplied only when the impl is `OAuth2Auth`. Fresh clock + empty transport, so
/// no refresh POST is sent.
fn apply(
    auth_impl: &dyn Auth,
    store: &dyn CredStore,
    oauth: Option<&OAuthConfig>,
) -> Result<WireRequest, CanonicalError> {
    let header = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let beta: Vec<(&str, &str)> = Vec::new();
    let ctx = crate::ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        beta_headers: &beta,
    };
    let amb = ambient_spec();
    let authc = AuthCtx {
        store_key: "prov",
        inline_key: None,
        api_header: Some(&header),
        oauth,
        ambient: Some(&amb),
    };
    let clock = FakeClock::new(0);
    let tx = MockTransport::ok(vec![]);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    auth_impl.apply(&mut wire, &ctx, &authc, store, &clock, &tx)?;
    Ok(wire)
}

fn oauth_cfg() -> OAuthConfig {
    OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: None,
        client_id: "cid".into(),
        scope: None,
        beta_headers: vec![],
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
        system_preamble: None,
    }
}

#[test]
fn static_secret_uses_the_discovered_cred_when_the_store_misses() {
    // Zero-setup: no stored cred and no --api-key, but the row's ambient source is
    // discovered. The discovered secret is written exactly as a stored one would be.
    let store = MemoryCredStore::with_ambient(Cred::Bearer {
        token: Secret::new("ambient-tok"),
    });
    let wire = apply(&StaticSecretAuth, &store, None).unwrap();
    assert_eq!(wire.header("Authorization"), Some("Bearer ambient-tok"));
}

#[test]
fn ambient_named_but_absent_is_missing_creds_77() {
    // The row names an ambient source but the box holds none (`discover` → None):
    // the empty case folds back to MissingCreds → 77, identical to naming no source.
    let store = MemoryCredStore::new();
    let err = apply(&StaticSecretAuth, &store, None).unwrap_err();
    assert_eq!(err.exit_code(), 77);
}

#[test]
fn stored_cred_shadows_the_ambient_one() {
    // `get` is tried before `discover`: a logged-in cred wins, so a later `bz login`
    // overrides whatever a foreign tool left on the box. Both sources are present.
    let store = MemoryCredStore::with(
        "prov",
        Cred::Bearer {
            token: Secret::new("stored-tok"),
        },
    );
    let wire = apply(&StaticSecretAuth, &store, None).unwrap();
    assert_eq!(wire.header("Authorization"), Some("Bearer stored-tok"));
}

#[test]
fn oauth_runs_from_an_ambient_cred_with_no_login() {
    // The real zero-setup Claude Code path: no stored cred, an ambient OAuth2 cred
    // discovered, fresh ⇒ no refresh POST, the discovered bearer straight on the wire.
    let cred = Cred::OAuth2 {
        access_token: Secret::new("at-ambient"),
        refresh_token: Secret::new("rt"),
        expires_at: 10_000,
        scope: None,
        account_id: None,
    };
    let store = MemoryCredStore::with_ambient(cred);
    let cfg = oauth_cfg();
    let wire = apply(&OAuth2Auth, &store, Some(&cfg)).unwrap();
    assert_eq!(wire.header("Authorization"), Some("Bearer at-ambient"));
}

#[test]
fn oauth_with_neither_stored_nor_ambient_is_not_logged_in_77() {
    // Both credential sources empty: the not-logged-in error (→77).
    let store = MemoryCredStore::new();
    let cfg = oauth_cfg();
    let err = apply(&OAuth2Auth, &store, Some(&cfg)).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("not logged in"));
}

#[test]
fn an_ambient_block_deserializes_from_toml_and_resolves_onto_the_row() {
    // The `format = "claude_code"` snake_case spelling parses to the enum, and
    // resolution carries the whole block onto the row `apply` reads (the data path
    // a shipped OAuth row uses to opt into zero-setup discovery).
    let toml = concat!(
        "[[provider]]\n",
        "name = \"anth-oauth\"\n",
        "base_url = \"https://api.anthropic.com\"\n",
        "protocol = \"anthropic_messages\"\n",
        "auth = \"api_key\"\n",
        "api_header = { name = \"x-api-key\", scheme = \"raw\" }\n",
        "ambient = { format = \"claude_code\", path = \"~/.claude/.credentials.json\" }\n",
    );
    let file = crate::parse_config(toml).unwrap();
    let selector = PartialConfig {
        provider: Some("anth-oauth".into()),
        ..Default::default()
    };
    let cfg = selector.or(file).into_resolved(None).unwrap();
    assert_eq!(cfg.provider.ambient, Some(ambient_spec()));
}
