//! The auth seam (arch §4.1, auth §1.2): the `Auth` trait — the ONLY data-plane
//! consumer of `CredStore`/`Clock` — plus `AuthCtx`, the auth-private projection
//! that carries the credential-store key and inline secret so they are TYPE-LEVEL
//! unreachable from `Protocol::encode` (the §6.5 stateless boundary). The two
//! staleness-free impls (`ApiKeyAuth`/`BearerAuth`) live here; `OAuth2` and the
//! pure OAuth builders land with their own task.

use serde::Deserialize;

use crate::canonical::{CanonicalError, ErrorKind};
use crate::config::provider::{HeaderScheme, HeaderSpec};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::store::{Clock, Cred, CredStore, Secret};
use crate::transport::Transport;

/// Produce the finished auth headers on a `WireRequest`, given a store and a clock
/// (arch §4.1). The ONLY function permitted to touch credentials or the clock —
/// everything before and after it is a pure fn of `(bytes_in, ResolvedConfig)`.
/// Object-safe; called once by `run` between `encode` and `transport.send`.
pub trait Auth: Send + Sync {
    fn apply(
        &self,
        wire: &mut WireRequest,
        ctx: &ProviderCtx,
        auth: &AuthCtx,
        store: &dyn CredStore,
        clock: &dyn Clock,
        transport: &dyn Transport,
    ) -> Result<(), CanonicalError>;
}

/// The auth-private projection handed ONLY to `Auth::apply`, never to
/// `Protocol::encode` (arch §4.1, auth §1.3). `store_key` is a `CredStore` key,
/// never matched on; `inline_key` is the §6.5 inline-key bypass; `oauth` is `Some`
/// exactly when the resolved row is `AuthId::OAuth2` (a resolve invariant).
pub struct AuthCtx<'a> {
    pub store_key: &'a str,
    pub inline_key: Option<&'a Secret>,
    pub oauth: Option<&'a OAuthConfig>,
}

/// The OAuth auth-row as data (auth §7.1): endpoints, `client_id`, `scope`, and
/// the auth-mode-dependent `beta_headers` (e.g. `anthropic-beta: oauth-…`). The
/// provider NAME is deliberately absent — it lives once, as the row key /
/// `store_key`. The pure OAuth builders take `&OAuthConfig`, so no vendor policy
/// is compiled into the core.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OAuthConfig {
    pub authorize_url: String,
    pub token_url: String,
    #[serde(default)]
    pub device_url: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub beta_headers: Vec<(String, String)>,
}

/// Write `secret` into the auth header the row's `HeaderSpec` names (auth §2):
/// one data-driven operation shared by every secret-bearing impl. The `match` on
/// `HeaderScheme` is value formatting (two total arms), never a vendor branch —
/// `x-api-key`/`x-goog-api-key` are `Raw`, `Authorization` is `Bearer`. The value
/// is never logged; `Secret::expose` is the single read site.
fn set_auth_header(wire: &mut WireRequest, spec: &HeaderSpec, secret: &Secret) {
    let value = match spec.scheme {
        HeaderScheme::Raw => secret.expose().to_owned(),
        HeaderScheme::Bearer => format!("Bearer {}", secret.expose()),
    };
    wire.set_header(&spec.name, &value);
}

/// An `Auth` failure (arch §8 → exit 77). The `message` differs by what would fix
/// it; the `kind` is always `Auth`.
fn auth_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Auth,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// The secret for `ApiKey`/`Bearer`, in precedence order (auth §3.1): the resolved
/// `inline_key` if present (the §6.5 stateless bypass — the store is never read),
/// else the matching `Cred` from `store.get(store_key)`, else `MissingCreds` → 77.
/// A stored `OAuth2` cred under an api-key/bearer row is config drift, surfaced as
/// a distinct error rather than a silent fallthrough.
fn resolved_secret(store: &dyn CredStore, auth: &AuthCtx) -> Result<Secret, CanonicalError> {
    if let Some(inline) = auth.inline_key {
        return Ok(inline.clone());
    }
    match store.get(auth.store_key) {
        Some(Cred::ApiKey { key }) => Ok(key),
        Some(Cred::Bearer { token }) => Ok(token),
        Some(Cred::OAuth2 { .. }) => Err(auth_error(
            "stored credential is OAuth2 but this provider is configured for an \
             API key / bearer token; reconfigure the row or re-run `bz login`",
        )),
        None => Err(auth_error(
            "no credential for this provider: set BRAZEN_API_KEY (or the provider \
             API-key env var / --api-key) or run `bz login`",
        )),
    }
}

/// The staleness-free auth (auth §3.1): resolve the secret and write it into the
/// header the row's `HeaderSpec` names. ONE impl behind both `AuthId::ApiKey` and
/// `AuthId::Bearer` — the two differ ONLY in `HeaderScheme`, which is row data
/// `set_auth_header` reads, so they are "not two dispatch sites" (auth §3.1); the
/// two `AuthId`s exist purely so config names the intent (`api_key` vs `bearer`).
/// "Refresh if stale" has an empty case for a secret that never goes stale, so
/// `clock`/`transport` are unused — this *is* that empty case, not a special one.
pub struct StaticSecretAuth;

impl Auth for StaticSecretAuth {
    fn apply(
        &self,
        wire: &mut WireRequest,
        ctx: &ProviderCtx,
        auth: &AuthCtx,
        store: &dyn CredStore,
        _clock: &dyn Clock,
        _transport: &dyn Transport,
    ) -> Result<(), CanonicalError> {
        let secret = resolved_secret(store, auth)?;
        set_auth_header(wire, ctx.api_header, &secret);
        Ok(())
    }
}
