//! Silent refresh — the only stateful thing in a normal run (auth §6). `OAuth2Auth`
//! is the sole `Auth` impl that uses `clock` and `transport`: it detects staleness
//! with a pure clock comparison, refreshes over the SAME `Transport` seam, persists
//! the new token (persist-then-use), and writes the bearer header plus the
//! auth-mode-dependent beta headers (§4). A failed refresh / not-logged-in is 77.

use super::oauth::{is_expired, parse_token_response, Grant};
use super::wire::build_token_exchange_request;
use super::{auth_error, require_header, set_auth_header, Auth, AuthCtx};
use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::store::{Clock, Cred, CredStore};
use crate::transport::{Transport, TransportResponse};

/// The OAuth2 data-plane auth (auth §6). OAuth knowledge — refresh, the bearer
/// header, AND the `anthropic-beta: oauth-…` header — is fully contained here; the
/// registry shares one `&OAuth2Auth` across every OAuth row (it reads endpoints /
/// `client_id` / `scope` / `beta_headers` from the `OAuthConfig` on `AuthCtx`).
pub struct OAuth2Auth;

impl Auth for OAuth2Auth {
    fn apply(
        &self,
        wire: &mut WireRequest,
        _ctx: &ProviderCtx,
        auth: &AuthCtx,
        store: &dyn CredStore,
        clock: &dyn Clock,
        transport: &dyn Transport,
    ) -> Result<(), CanonicalError> {
        // Defensive, not a live branch: resolution pairs an `oauth2` row with a
        // present `OAuthConfig` or fails at resolve (§1.3); exercised by a direct
        // unit test handing `oauth: None`, proving the no-panic contract → 78.
        let cfg = auth.oauth.ok_or_else(oauth_row_misconfigured)?;
        let Some(Cred::OAuth2 {
            access_token,
            refresh_token,
            expires_at,
            scope,
            account_id,
        }) = store.get(auth.store_key)
        else {
            return Err(auth_error(
                "not logged in for this provider: run `bz login <provider>`",
            ));
        };

        let token = if is_expired(expires_at, clock.now()) {
            let mut req = build_token_exchange_request(
                cfg,
                Grant::Refresh {
                    refresh_token: &refresh_token,
                },
            );
            // The refresh POST shares the data request's hang risk, so it inherits
            // the same resolved transport bounds `run` stamped on `wire` (config §4).
            req.timeouts = wire.timeouts;
            let bytes = collect_body(transport.send(req)?)?;
            let fresh = parse_token_response(&bytes, clock.now()).map_err(|_| {
                auth_error("token refresh failed (revoked or expired): run `bz login <provider>`")
            })?;
            store
                .put(
                    auth.store_key,
                    &fresh.as_cred(&refresh_token, &scope, &account_id),
                )
                .map_err(persist_failed)?;
            fresh.access_token
        } else {
            access_token
        };

        set_auth_header(wire, require_header(auth)?, &token);
        for (name, value) in &cfg.beta_headers {
            wire.set_header(name, value);
        }
        // The auth-mode-dependent header whose VALUE is the credential's account id
        // (auth §10.4): NAME is row data, value is the cred fact. Both absent ⇒ no
        // header (Anthropic). The account does not change on refresh, so the stored
        // value is correct regardless of which branch produced `token`.
        if let (Some(name), Some(id)) = (cfg.account_header.as_deref(), account_id.as_deref()) {
            wire.set_header(name, id);
        }
        Ok(())
    }
}

/// Drain a transport response body to a `Vec` (auth §6): the refresh `POST` answers
/// on the SAME seam as the data request, so a mid-stream read failure is the
/// transport's own `Transport` error (→69), distinct from a parsed `invalid_grant`.
pub(crate) fn collect_body(resp: TransportResponse) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::new();
    for chunk in resp.body {
        let bytes = chunk.map_err(|e| CanonicalError {
            kind: ErrorKind::Transport,
            message: format!("transport error reading token response: {e}"),
            provider_detail: None,
        })?;
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

/// A resolved run never reaches `apply` with `oauth: None` (§1.3); a `Config` error
/// (→78) is the defensive surface if it somehow does.
fn oauth_row_misconfigured() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: "oauth2 provider row has no oauth config (should be caught at resolve)".to_owned(),
        provider_detail: None,
    }
}

/// A failure to persist the refreshed token (auth §6.2): surfaced as an auth error
/// (→77) — the credential subsystem could not record the new token.
fn persist_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Auth,
        message: format!("could not persist refreshed credential: {e}"),
        provider_detail: None,
    }
}
