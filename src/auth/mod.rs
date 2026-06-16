//! The auth seam (arch §4.1, auth §1.2): the `Auth` trait — the ONLY data-plane
//! consumer of `CredStore`/`Clock` — plus `AuthCtx`, the auth-private projection
//! that carries the credential-store key and inline secret so they are TYPE-LEVEL
//! unreachable from `Protocol::encode` (the §6.5 stateless boundary). The three
//! impls (ApiKey/Bearer/OAuth2) and the pure OAuth builders land with their tasks.

use serde::Deserialize;

use crate::canonical::CanonicalError;
use crate::protocol::{ProviderCtx, WireRequest};
use crate::store::{Clock, CredStore, Secret};
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
