//! The auth seam (arch §4.1, auth §1.2): the `Auth` trait — the ONLY data-plane
//! consumer of `CredStore`/`Clock` — plus `AuthCtx`, the auth-private projection
//! that carries the credential-store key and inline secret so they are TYPE-LEVEL
//! unreachable from `Protocol::encode` (the §6.5 stateless boundary). The two
//! staleness-free impls (`ApiKeyAuth`/`BearerAuth`) live here; the OAuth row DATA
//! (`OAuthConfig`/`RedirectSpec`) in `oauth_row`; the `OAuth2` impl, the pure OAuth
//! builders/parsers, and the `bz --login` control plane live in the
//! `oauth`/`wire`/`refresh`/`login` submodules.

mod flows;
mod jwt;
pub mod login;
pub mod oauth;
mod oauth_row;
pub mod refresh;
mod urlencode;
pub mod wire;

pub use oauth_row::{OAuthConfig, RedirectSpec};
pub use refresh::OAuth2Auth;
/// The OAuth query codec, reused by the model-discovery GET to URL-encode a
/// `[provider.models]` `query` (model-discovery §3.2) — CLI-reachable, so ungated.
pub(crate) use urlencode::encode_pairs;
pub use wire::query_from_request_line;

// CLI-unreachable: these feed only the `#[cfg(test)]` lib prelude (the pure OAuth
// builders/parsers the data plane reaches via `OAuth2Auth`/`refresh`, never by name).
// Gated so the redundant root re-export is not dead code in the release build (§9.8).
#[cfg(test)]
pub(crate) use oauth::{is_expired, parse_token_response, AuthError, Grant, TokenResponse};
#[cfg(test)]
pub(crate) use wire::{build_authorize_url, build_token_exchange_request, parse_callback, Pkce};

use crate::canonical::{CanonicalError, ErrorKind};
use crate::config::provider::{HeaderScheme, HeaderSpec};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::store::{AmbientSpec, Clock, Cred, CredStore, Secret};
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
/// never matched on; `inline_key` is the §6.5 inline-key bypass; `api_header` is
/// `Some` for every keyed row and `None` exactly for `AuthId::None`; `oauth` is
/// `Some` exactly when the resolved row is `AuthId::OAuth2` (both resolve invariants);
/// `ambient` is `Some` exactly when the row carries an `ambient` block (auth §5.5) —
/// the zero-setup discovery source consulted on a store miss.
pub struct AuthCtx<'a> {
    pub store_key: &'a str,
    pub inline_key: Option<&'a Secret>,
    pub api_header: Option<&'a HeaderSpec>,
    pub oauth: Option<&'a OAuthConfig>,
    pub ambient: Option<&'a AmbientSpec>,
}

/// The auth header for a keyed row, or a defensive `Config` error (→78) if absent.
/// Resolution guarantees `api_header.is_some()` for every non-`None` auth row, so
/// the `None` arm is not a live branch — it is the no-panic surface for a row that
/// somehow reached a keyed impl without one (exercised directly in a unit test, like
/// `oauth_row_misconfigured`).
pub(crate) fn require_header<'a>(auth: &AuthCtx<'a>) -> Result<&'a HeaderSpec, CanonicalError> {
    auth.api_header.ok_or_else(|| CanonicalError {
        kind: ErrorKind::Config,
        message: "keyed provider row has no api_header (should be caught at resolve)".to_owned(),
        provider_detail: None,
    })
}

/// The keyless auth (auth §3.1): a row whose provider needs no credential — local
/// Ollama is the shipped case. It reads no `CredStore`, writes no header, and so
/// uses none of `Auth::apply`'s seams. A present `--api-key`/stored cred is simply
/// ignored (the row declares the header is not wanted), the keyless dual of the
/// keyed impls' "missing key → 77".
pub struct NoAuth;

impl Auth for NoAuth {
    fn apply(
        &self,
        _wire: &mut WireRequest,
        _ctx: &ProviderCtx,
        _auth: &AuthCtx,
        _store: &dyn CredStore,
        _clock: &dyn Clock,
        _transport: &dyn Transport,
    ) -> Result<(), CanonicalError> {
        Ok(())
    }
}

/// Write `secret` into the auth header the row's `HeaderSpec` names (auth §2):
/// one data-driven operation shared by every secret-bearing impl. The `match` on
/// `HeaderScheme` is value formatting (two total arms), never a vendor branch —
/// `x-api-key`/`x-goog-api-key` are `Raw`, `Authorization` is `Bearer`. The value
/// is never logged; `Secret::expose` is the single read site.
pub(crate) fn set_auth_header(wire: &mut WireRequest, spec: &HeaderSpec, secret: &Secret) {
    let value = match spec.scheme {
        HeaderScheme::Raw => secret.expose().to_owned(),
        HeaderScheme::Bearer => format!("Bearer {}", secret.expose()),
    };
    wire.set_header(&spec.name, &value);
}

/// An `Auth` failure (arch §8 → exit 77). The `message` differs by what would fix
/// it; the `kind` is always `Auth`.
pub(crate) fn auth_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Auth,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// Where a credential comes from, in one place (auth §5.5): the store under
/// `store_key`, else — on a miss — the row's ambient discovery source (`bz`'s
/// `discover` reads Claude Code's `~/.claude/.credentials.json`). A row with no
/// `ambient` block makes the second arm `None`: the general path with an empty
/// input, not a special case. Both `Auth` impls fetch through here, so "stored vs
/// discovered" is a single decision, never duplicated.
pub(crate) fn fetch_cred(store: &dyn CredStore, auth: &AuthCtx) -> Option<Cred> {
    store
        .get(auth.store_key)
        .or_else(|| auth.ambient.and_then(|spec| store.discover(spec)))
}

/// The secret for `ApiKey`/`Bearer`, in precedence order (auth §3.1): the resolved
/// `inline_key` if present (the §6.5 stateless bypass — the store is never read),
/// else the [`fetch_cred`] credential (stored, else ambient-discovered), else
/// `MissingCreds` → 77. A stored `OAuth2` cred under an api-key/bearer row is config
/// drift, surfaced as a distinct error rather than a silent fallthrough.
fn resolved_secret(store: &dyn CredStore, auth: &AuthCtx) -> Result<Secret, CanonicalError> {
    if let Some(inline) = auth.inline_key {
        return Ok(inline.clone());
    }
    match fetch_cred(store, auth) {
        Some(Cred::ApiKey { key }) => Ok(key),
        Some(Cred::Bearer { token }) => Ok(token),
        Some(Cred::OAuth2 { .. }) => Err(auth_error(
            "stored credential is OAuth2 but this provider is configured for an \
             API key / bearer token; reconfigure the row or re-run `bz --login --provider <id>`",
        )),
        None => Err(auth_error(
            "no credential for this provider: set BRAZEN_API_KEY (or the provider \
             API-key env var / --api-key) or run `bz --login --provider <id>`",
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
        _ctx: &ProviderCtx,
        auth: &AuthCtx,
        store: &dyn CredStore,
        _clock: &dyn Clock,
        _transport: &dyn Transport,
    ) -> Result<(), CanonicalError> {
        let secret = resolved_secret(store, auth)?;
        set_auth_header(wire, require_header(auth)?, &secret);
        Ok(())
    }
}
