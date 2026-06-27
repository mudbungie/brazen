//! The v0.1 data-plane auth (auth §3.1, §8): `StaticSecretAuth::apply` behind the `api_key`/`bearer` ids.
//! Secret resolution order (inline_key → store.get → MissingCreds/77), the
//! data-driven header write (`x-api-key` Raw, `Authorization: Bearer`, the
//! no-vendor-branch `x-goog-api-key`), and the inline-key bypass that never reads
//! the store. `clock`/`transport` are the empty case here — refresh is `OAuth2`'s.

use std::io;

use crate::testing::{FakeClock, MemoryCredStore, MockTransport};
use crate::{
    AmbientSpec, Auth, AuthCtx, CanonicalError, Cred, CredStore, ErrorKind, HeaderScheme,
    HeaderSpec, NoAuth, ProviderCtx, Registry, Secret, StaticSecretAuth, WireRequest,
};

/// Run `auth_impl.apply` with a fresh wire against the given `HeaderSpec` and
/// auth/store, returning the mutated `WireRequest`. `clock`/`transport` are wired
/// but unused by the staleness-free impls.
fn apply(
    auth_impl: &dyn Auth,
    spec: HeaderSpec,
    auth: &AuthCtx,
    store: &dyn CredStore,
) -> Result<WireRequest, CanonicalError> {
    let beta: Vec<(&str, &str)> = Vec::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        beta_headers: &beta,
    };
    // The auth header rides `AuthCtx` now; inject the test's `spec`, keeping the
    // caller's store_key/inline_key/oauth.
    let authc = AuthCtx {
        api_header: Some(&spec),
        ..*auth
    };
    let clock = FakeClock::new(0);
    let transport = MockTransport::ok(vec![]);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    auth_impl.apply(&mut wire, &ctx, &authc, store, &clock, &transport)?;
    Ok(wire)
}

/// A store that must never be touched: proves the inline-key path reads no creds.
struct PanicStore;
impl CredStore for PanicStore {
    fn get(&self, _: &str) -> Option<Cred> {
        panic!("inline-key path must not read the store");
    }
    fn put(&self, _: &str, _: &Cred) -> io::Result<()> {
        panic!("inline-key path must not write the store");
    }
    fn discover(&self, _: &AmbientSpec) -> Option<Cred> {
        panic!("inline-key path must not discover an ambient cred");
    }
}

fn ctx_for(store_key: &str) -> AuthCtx<'_> {
    AuthCtx {
        store_key,
        inline_key: None,
        api_header: None,
        oauth: None,
        ambient: None,
    }
}

#[test]
fn api_key_writes_raw_header_from_store() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-test"),
        },
    );
    // Dispatch the registered impl by id — the pipeline's path, no name match.
    let auth_impl = Registry::builtin().auth(crate::AuthId::ApiKey);
    let spec = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let wire = apply(auth_impl, spec, &ctx_for("anthropic"), &store).unwrap();
    // Golden header bytes: bare secret, single header, exact (name, value).
    assert_eq!(
        wire.headers,
        vec![("x-api-key".to_string(), "sk-test".to_string())]
    );
}

#[test]
fn bearer_writes_authorization_bearer_from_store() {
    let store = MemoryCredStore::with(
        "openai",
        Cred::Bearer {
            token: Secret::new("tok-9"),
        },
    );
    let auth_impl = Registry::builtin().auth(crate::AuthId::Bearer);
    let spec = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let wire = apply(auth_impl, spec, &ctx_for("openai"), &store).unwrap();
    // Golden: the "Bearer " prefix is the scheme's doing, not a vendor branch.
    assert_eq!(
        wire.headers,
        vec![("Authorization".to_string(), "Bearer tok-9".to_string())]
    );
}

#[test]
fn raw_scheme_names_any_header_with_no_vendor_branch() {
    // Google's x-goog-api-key is the same Raw impl against a row that names it.
    let store = MemoryCredStore::with(
        "google",
        Cred::ApiKey {
            key: Secret::new("goog-1"),
        },
    );
    let spec = HeaderSpec {
        name: "x-goog-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let wire = apply(&StaticSecretAuth, spec, &ctx_for("google"), &store).unwrap();
    assert_eq!(
        wire.headers,
        vec![("x-goog-api-key".to_string(), "goog-1".to_string())]
    );
}

#[test]
fn inline_key_beats_store_and_never_reads_it() {
    let inline = Secret::new("inline-key");
    let auth = AuthCtx {
        store_key: "anthropic",
        inline_key: Some(&inline),
        api_header: None,
        oauth: None,
        ambient: None,
    };
    let spec = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    // PanicStore would panic if read — proving the §6.5 stateless bypass.
    let wire = apply(&StaticSecretAuth, spec, &auth, &PanicStore).unwrap();
    assert_eq!(wire.header("x-api-key"), Some("inline-key"));
}

#[test]
fn missing_cred_is_auth_error_77() {
    let store = MemoryCredStore::new();
    let spec = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let err = apply(&StaticSecretAuth, spec, &ctx_for("anthropic"), &store).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth);
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("bz login"));
}

#[test]
fn oauth_cred_under_api_key_row_is_wrong_kind_77() {
    // Config drift: a stored OAuth cred for a row reconfigured to api_key.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::OAuth2 {
            access_token: Secret::new("at"),
            refresh_token: Secret::new("rt"),
            expires_at: 1_700_000_000,
            scope: None,
            account_id: None,
        },
    );
    let spec = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let err = apply(&StaticSecretAuth, spec, &ctx_for("anthropic"), &store).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth);
    assert_eq!(err.exit_code(), 77);
    assert!(err.message.contains("OAuth2"));
    // No silent fallthrough: nothing was written to the wire.
}

/// Apply `auth_impl` against an explicit `AuthCtx` (no `api_header` injection), so a
/// test can drive the keyless / missing-header paths directly.
fn apply_with(
    auth_impl: &dyn Auth,
    authc: &AuthCtx,
    store: &dyn CredStore,
) -> Result<WireRequest, CanonicalError> {
    let beta: Vec<(&str, &str)> = Vec::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        beta_headers: &beta,
    };
    let clock = FakeClock::new(0);
    let transport = MockTransport::ok(vec![]);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    auth_impl.apply(&mut wire, &ctx, authc, store, &clock, &transport)?;
    Ok(wire)
}

#[test]
fn no_auth_writes_no_header_and_reads_no_store() {
    // `auth = "none"` (local Ollama): keyless. No api_header, no credential — apply
    // still succeeds and the wire is untouched. `PanicStore` proves the store is
    // never read, the keyless dual of the keyed impls' "missing key → 77".
    let authc = AuthCtx {
        store_key: "ollama",
        inline_key: None,
        api_header: None,
        oauth: None,
        ambient: None,
    };
    let wire = apply_with(&NoAuth, &authc, &PanicStore).unwrap();
    assert!(wire.headers.is_empty());
}

#[test]
fn keyed_row_without_api_header_is_config_error_78() {
    // Defensive, not a live branch: resolution guarantees a keyed row carries an
    // `api_header`; if one slips through with `api_header: None`, the keyed impl
    // surfaces a `Config` error (→78), never a panic — the dual of
    // `oauth_row_misconfigured`. An inline key gets past secret resolution so the
    // missing-header check is what fires.
    let inline = Secret::new("k");
    let authc = AuthCtx {
        store_key: "p",
        inline_key: Some(&inline),
        api_header: None,
        oauth: None,
        ambient: None,
    };
    let err = apply_with(&StaticSecretAuth, &authc, &PanicStore).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
}
