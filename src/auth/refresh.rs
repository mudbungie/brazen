//! Silent refresh — the only stateful thing in a normal run (auth §6). `OAuth2Auth`
//! is the sole `Auth` impl that uses `clock` and `transport`: it detects staleness
//! with a pure clock comparison, refreshes OWNED state over the SAME `Transport`
//! seam, persists the new token (persist-then-use), and writes the bearer header
//! plus auth-mode-dependent beta headers (§4). Borrowed ambient state is read-only:
//! stale returns 77 before transport or persistence.
//!
//! The row's credential SOURCES are a list read in order, not a single verdict
//! (auth §5.5, §6.2): the store first, then the ambient block — and the ambient
//! block gets its look on a store MISS *and* on a store hit this run could not make
//! fresh, because a refresh the authorization server refuses is permanent for that
//! credential and says nothing about a tool on this box that is signed in right now.

use super::oauth::{is_expired, parse_token_response, Grant};
use super::wire::build_token_exchange_request;
use super::OAuthConfig;
use super::{auth_error, fetch_cred, require_header, set_auth_header, Auth, AuthCtx, CredSource};
use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::store::{Clock, Cred, CredStore, Secret};
use crate::transport::{Transport, TransportResponse};

/// The bearer token for one request plus the account id that rides beside it (§10.4).
type Bearer = (Secret, Option<String>);

/// The stale OWNED credential's carry-forward fields (auth §6.2): a refresh response
/// that omits a rotated `refresh_token`, a `scope`, or the `id_token` the account id
/// came from keeps the prior value — the account does not change on refresh.
struct Prior {
    refresh_token: Secret,
    scope: Option<String>,
    account_id: Option<String>,
}

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
        let (token, account_id) = bearer(wire, auth, store, clock, transport, cfg)?;
        set_auth_header(wire, require_header(auth)?, &token);
        for (name, value) in &cfg.beta_headers {
            wire.set_header(name, value);
        }
        // The auth-mode-dependent header whose VALUE is the credential's account id
        // (auth §10.4): NAME is row data, value is the cred fact. Both absent ⇒ no
        // header (Anthropic). The account does not change on refresh, so the value
        // is correct regardless of which source produced `token`.
        if let (Some(name), Some(id)) = (cfg.account_header.as_deref(), account_id.as_deref()) {
            wire.set_header(name, id);
        }
        Ok(())
    }
}

/// Walk the row's credential sources for a token this request may send (auth §5.5,
/// §6). A fresh credential from either source is used as-is; a stale OWNED one is
/// refreshed, persisted and used; a stale BORROWED one stops here, because refresh
/// authority stays with the tool that wrote the file — adopting its rotation would
/// make two owners for one credential and shadow future discovery.
fn bearer(
    wire: &WireRequest,
    auth: &AuthCtx,
    store: &dyn CredStore,
    clock: &dyn Clock,
    transport: &dyn Transport,
    cfg: &OAuthConfig,
) -> Result<Bearer, CanonicalError> {
    let Some(fetched) = fetch_cred(store, auth) else {
        return Err(not_logged_in());
    };
    let Cred::OAuth2 {
        access_token,
        refresh_token,
        expires_at,
        scope,
        account_id,
    } = fetched.cred
    else {
        return Err(not_logged_in());
    };
    if !is_expired(expires_at, clock.now()) {
        return Ok((access_token, account_id));
    }
    match fetched.source {
        CredSource::Borrowed(path) => Err(borrowed_expired(&path)),
        CredSource::Owned => {
            let prior = Prior {
                refresh_token,
                scope,
                account_id,
            };
            refresh(wire, auth, store, clock, transport, cfg, &prior)
        }
    }
}

/// Refresh an OWNED stale credential (auth §6): build the exchange request, send it
/// over the SAME transport seam as the data request, persist the result BEFORE using
/// it (so the next process starts fresh), and hand back the new bearer. A response
/// the token parser cannot read is a refusal — see [`spent`].
fn refresh(
    wire: &WireRequest,
    auth: &AuthCtx,
    store: &dyn CredStore,
    clock: &dyn Clock,
    transport: &dyn Transport,
    cfg: &OAuthConfig,
    prior: &Prior,
) -> Result<Bearer, CanonicalError> {
    let mut req = build_token_exchange_request(
        cfg,
        Grant::Refresh {
            refresh_token: &prior.refresh_token,
        },
    );
    // The refresh POST shares the data request's hang risk AND its wire identity, so
    // it inherits the whole transport policy `run` stamped on `wire` (config §4,
    // transport §4.3): the bounds, and the operator's transport delegate when the row
    // selects one — the auth control request must not fall back to a different HTTP
    // stack than the data request.
    req.timeouts = wire.timeouts;
    req.exec = wire.exec.clone();
    let bytes = collect_body(transport.send(req)?)?;
    let Ok(fresh) = parse_token_response(&bytes, clock.now()) else {
        return spent(store, auth, clock);
    };
    store
        .put(
            auth.store_key,
            &fresh.as_cred(&prior.refresh_token, &prior.scope, &prior.account_id),
        )
        .map_err(persist_failed)?;
    Ok((fresh.access_token, prior.account_id.clone()))
}

/// The stored credential is spent: ANY refresh response the token parser cannot read
/// lands here — a permanent `invalid_grant` OR a transient token-endpoint 503/429
/// that still returns a body. `apply` does NOT peek `resp.status` to tell them apart
/// (auth §6.2); the operator's corrective action is the same either way.
///
/// What differs is whether this box has a SECOND source. When the row names an
/// ambient block, the refusal is not the end of the run: the same discovery the store
/// miss would have done runs here, and a FRESH borrowed credential answers the
/// request (a stale one does not — brazen may not refresh that either). Only when no
/// source answers is it an error, and then the message names the file, because
/// "re-run `bz --login`" is the wrong instruction for a box whose other tool holds a
/// working sign-in. With no ambient block the message is the original one, and stays
/// deliberately non-alarming: it does not assert the credential is revoked, and a
/// transient fault is retryable — retry is the caller's job, not `bz`'s.
fn spent(
    store: &dyn CredStore,
    auth: &AuthCtx,
    clock: &dyn Clock,
) -> Result<Bearer, CanonicalError> {
    let Some(spec) = auth.ambient else {
        return Err(auth_error(
            "token refresh failed; re-run `bz --login --provider <id>` if this persists",
        ));
    };
    if let Some(Cred::OAuth2 {
        access_token,
        expires_at,
        account_id,
        ..
    }) = store.discover(spec)
    {
        if !is_expired(expires_at, clock.now()) {
            return Ok((access_token, account_id));
        }
    }
    Err(auth_error(&format!(
        "token refresh failed, and the ambient credential at {} is absent or expired; \
         sign in with the tool that owns that file, or re-run `bz --login --provider <id>`",
        spec.path
    )))
}

/// No credential from any source (auth §6) — including a stored cred of the wrong
/// shape under an OAuth row, which is config drift, not a login.
fn not_logged_in() -> CanonicalError {
    auth_error(
        "not logged in for this provider: run `bz --login --provider <id>` (or \
         sign in to a tool whose ambient credential this row discovers)",
    )
}

/// A discovered credential that is already stale (auth §6.2): brazen refreshes only
/// what it owns, so the fix belongs to the tool that wrote the file — and the message
/// NAMES that file, since "the tool that owns it" is not something an operator can
/// look up. The path rides `CredSource::Borrowed`, from the read that knew it.
fn borrowed_expired(path: &str) -> CanonicalError {
    auth_error(&format!(
        "the ambient credential at {path} is expired; refresh it with the tool that owns \
         that file, or run `bz --login --provider <id>` to hold one of brazen's own"
    ))
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
            retry_after_seconds: None,
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
        retry_after_seconds: None,
    }
}

/// A failure to persist the refreshed token (auth §6.2): surfaced as an auth error
/// (→77) — the credential subsystem could not record the new token.
fn persist_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Auth,
        message: format!("could not persist refreshed credential: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
