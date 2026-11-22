//! The loopback AuthCode flow (auth §7.4) and the token exchange that ends BOTH
//! non-refresh logins, driven by [`login`](super::login). The headless device-code
//! variants live beside it in [`device`](super::device); they finish through this
//! file's [`exchange_auth_code`] whenever their variant answers a code rather than
//! a token. The interactive impurities (`BrowserLauncher`/`CodeReceiver`/`Pacer`)
//! arrive injected via [`LoginIo`](super::login::LoginIo), so both run offline in
//! tests, and the pure builders they share live in [`wire`](super::wire).

use super::login::LoginIo;
use super::oauth::{parse_token_response, AuthError, Grant, TokenResponse};
use super::refresh::collect_body;
use super::wire::{build_authorize_url, build_token_exchange_request, parse_callback, Pkce};
use super::{auth_error, OAuthConfig};
use crate::canonical::CanonicalError;
use crate::store::{Cred, Secret};
use crate::transport::TransportResponse;

/// AuthCode + loopback flow (RFC 8252 / auth §7.4, §10.1): bind the loopback on the
/// row's redirect port (`None` ⇒ ephemeral), build the PKCE-S256 authorize URL
/// against the row's redirect host/port/path, launch the browser, await the
/// callback, CSRF-check it, exchange the code, and return the cred.
pub(super) fn browser_flow(cfg: &OAuthConfig, io: &mut LoginIo) -> Result<Cred, CanonicalError> {
    let port = io
        .receiver
        .bind(cfg.redirect.port)
        .map_err(|e| auth_error(&format!("could not bind loopback listener: {e}")))?;
    let redirect_uri = format!("http://{}:{}{}", cfg.redirect.host, port, cfg.redirect.path);
    let pkce = Pkce::derive(io.verifier);
    let url = build_authorize_url(cfg, &pkce, io.state, &redirect_uri);
    io.browser
        .open(&url)
        .map_err(|e| auth_error(&format!("could not launch browser: {e}")))?;
    let query = io
        .receiver
        .await_query()
        .map_err(|e| auth_error(&format!("loopback receiver failed: {e}")))?;
    let callback = parse_callback(&query, io.state).map_err(fatal)?;
    let state = Some(callback.state.as_str());
    exchange_auth_code(
        cfg,
        io,
        &callback.code,
        &pkce.verifier,
        &redirect_uri,
        state,
    )
}

/// The AuthCode half-step both code-answering logins end in (auth §7.5): `POST
/// {token_url}` with `Grant::AuthCode`, then the ONE token parser. The loopback flow
/// reaches it with the code the browser redirected back and its own `redirect_uri`;
/// the Codex device variant reaches it with the code its poll answered and the
/// vendor's `deviceauth/callback` — one exchange, two ways of obtaining the code.
pub(super) fn exchange_auth_code(
    cfg: &OAuthConfig,
    io: &mut LoginIo,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    state: Option<&str>,
) -> Result<Cred, CanonicalError> {
    let req = build_token_exchange_request(
        cfg,
        Grant::AuthCode {
            code,
            verifier,
            redirect_uri,
            state,
        },
    );
    let tok = token_reply(io.transport.send(req)?, io.clock.now())?.map_err(fatal)?;
    Ok(tok.as_cred(&Secret::new(""), &None, &None))
}

/// Drain a token endpoint's answer and read it with the ONE parser (auth §7.5),
/// stamping the HTTP status onto a fatal one: the status is the transport's fact and
/// the sentence is the vendor's, and a failed sign-in prints both. A poll signal
/// passes through untouched.
pub(super) fn token_reply(
    resp: TransportResponse,
    now: u64,
) -> Result<Result<TokenResponse, AuthError>, CanonicalError> {
    let status = resp.status;
    let bytes = collect_body(resp)?;
    Ok(parse_token_response(&bytes, now).map_err(|err| match err {
        AuthError::Fatal(msg) => AuthError::Fatal(format!("HTTP {status}: {msg}")),
        signal => signal,
    }))
}

/// Any non-success token/callback outcome is fatal in the auth-code path (→77).
pub(super) fn fatal(err: AuthError) -> CanonicalError {
    let msg = match err {
        AuthError::Pending | AuthError::SlowDown => "unexpected poll signal".to_owned(),
        AuthError::Fatal(m) => m,
    };
    auth_error(&format!("login failed: {msg}"))
}
