//! The v0.1 data-plane auth (auth §3.1, §8): `StaticSecretAuth::apply` behind the `api_key`/`bearer` ids.
//! Secret resolution order (inline_key → store.get → MissingCreds/77), the
//! data-driven header write (`x-api-key` Raw, `Authorization: Bearer`, the
//! no-vendor-branch `x-goog-api-key`), and the inline-key bypass that never reads
//! the store. `clock`/`transport` are the empty case here — refresh is `OAuth2`'s.

use std::io;

use brazen::testing::{FakeClock, MemoryCredStore, MockTransport};
use brazen::{
    Auth, AuthCtx, CanonicalError, Cred, CredStore, ErrorKind, HeaderScheme, HeaderSpec,
    ProviderCtx, Registry, Secret, StaticSecretAuth, WireRequest,
};
use serde_json::{Map, Value};

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
    let extra: Map<String, Value> = Map::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        api_header: &spec,
        beta_headers: &beta,
        extra: &extra,
    };
    let clock = FakeClock::new(0);
    let transport = MockTransport::ok(vec![]);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    auth_impl.apply(&mut wire, &ctx, auth, store, &clock, &transport)?;
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
}

fn ctx_for(store_key: &str) -> AuthCtx<'_> {
    AuthCtx {
        store_key,
        inline_key: None,
        oauth: None,
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
    let auth_impl = Registry::builtin().auth(brazen::AuthId::ApiKey);
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
    let auth_impl = Registry::builtin().auth(brazen::AuthId::Bearer);
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
        oauth: None,
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
