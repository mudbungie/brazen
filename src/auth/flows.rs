//! The two `bz --login` OAuth flows (auth §7.3, §7.4), driven by [`login`](super::login)
//! and ending in one `Cred::OAuth2`. Device-code (RFC 8628, headless) and loopback
//! AuthCode (RFC 8252, `--browser`) share the pure builders in [`wire`](super::wire);
//! their interactive impurities (`BrowserLauncher`/`CodeReceiver`/`Pacer`) arrive
//! injected via [`LoginIo`](super::login::LoginIo), so both run offline in tests.

use serde::Deserialize;

use super::login::{config_err, LoginIo};
use super::oauth::{parse_token_response, AuthError, Grant};
use super::refresh::collect_body;
use super::wire::{
    build_authorize_url, build_token_exchange_request, form_post, parse_callback, Pkce,
};
use super::{auth_error, OAuthConfig};
use crate::canonical::CanonicalError;
use crate::store::{Cred, Secret};

/// Device-code flow (RFC 8628 / auth §7.3): request a device code, print the
/// `user_code` + `verification_uri` to STDERR, then poll the token endpoint every
/// `interval` s (`slow_down` adds 5 s cumulatively) until success, a fatal error,
/// or the `expires_in` deadline (→77). No browser, headless-friendly.
pub(super) fn device_flow(cfg: &OAuthConfig, io: &mut LoginIo) -> Result<Cred, CanonicalError> {
    let device_url = cfg.device_url.as_deref().ok_or_else(|| {
        config_err("this provider has no device endpoint; use `--browser`".to_owned())
    })?;
    let auth = parse_device_auth(&collect_body(
        io.transport
            .send(form_post(device_url, &device_params(cfg)))?,
    )?)?;
    let _ = writeln!(
        io.stderr,
        "To authorize, open {} and enter code: {}",
        auth.verification_uri, auth.user_code
    );

    let deadline = io.clock.now().saturating_add(auth.expires_in);
    let mut interval = auth.interval.unwrap_or(5);
    loop {
        if io.clock.now() >= deadline {
            return Err(auth_error(
                "device login expired before authorization; run `bz --login --provider <id>` again",
            ));
        }
        io.pacer.wait(interval);
        let req = build_token_exchange_request(
            cfg,
            Grant::Device {
                device_code: &auth.device_code,
            },
        );
        match parse_token_response(&collect_body(io.transport.send(req)?)?, io.clock.now()) {
            Ok(tok) => return Ok(tok.as_cred(&Secret::new(""), &None, &None)),
            Err(AuthError::Pending) => continue,
            Err(AuthError::SlowDown) => interval += 5,
            Err(AuthError::Fatal(msg)) => {
                return Err(auth_error(&format!("device login failed: {msg}")))
            }
        }
    }
}

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
    let req = build_token_exchange_request(
        cfg,
        Grant::AuthCode {
            code: &callback.code,
            verifier: &pkce.verifier,
            redirect_uri: &redirect_uri,
        },
    );
    let tok = parse_token_response(&collect_body(io.transport.send(req)?)?, io.clock.now())
        .map_err(fatal)?;
    Ok(tok.as_cred(&Secret::new(""), &None, &None))
}

/// The RFC 8628 device-authorization response (auth §7.3).
#[derive(Deserialize)]
struct DeviceAuth {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}

fn parse_device_auth(bytes: &[u8]) -> Result<DeviceAuth, CanonicalError> {
    serde_json::from_slice(bytes)
        .map_err(|e| auth_error(&format!("malformed device-authorization response: {e}")))
}

/// The device-authorization request params: `client_id` and `scope` when set.
fn device_params(cfg: &OAuthConfig) -> Vec<(&str, &str)> {
    let mut params = vec![("client_id", cfg.client_id.as_str())];
    if let Some(scope) = &cfg.scope {
        params.push(("scope", scope.as_str()));
    }
    params
}

/// Any non-success token/callback outcome is fatal in the auth-code path (→77).
fn fatal(err: AuthError) -> CanonicalError {
    let msg = match err {
        AuthError::Pending | AuthError::SlowDown => "unexpected poll signal".to_owned(),
        AuthError::Fatal(m) => m,
    };
    auth_error(&format!("login failed: {msg}"))
}
