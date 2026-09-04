//! The headless device-code logins (auth §7.3) — `bz --login` without `--browser`.
//! TWO wires reach the same `Cred::OAuth2`, and which one runs is a DATA read on the
//! row (`device.style`), never a vendor branch: RFC 8628 (the default) polls the
//! token endpoint with `Grant::Device`, while Codex's pre-standard variant (§10.8)
//! polls a `deviceauth` endpoint that answers an authorization CODE and finishes
//! through the ordinary AuthCode exchange in [`flows`](super::flows). Both share one
//! poll driver, so `interval`, the cumulative `slow_down`, and the expiry deadline
//! are decided once.

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::flows::exchange_auth_code;
use super::login::{config_err, LoginIo};
use super::oauth::{parse_token_response, AuthError, Grant};
use super::oauth_row::DeviceStyle;
use super::refresh::collect_body;
use super::wire::{build_token_exchange_request, form_post};
use super::{auth_error, OAuthConfig};
use crate::canonical::CanonicalError;
use crate::protocol::WireRequest;
use crate::store::{Cred, Secret};

/// The poll interval to use when the device-authorization response names none
/// (RFC 8628 §3.5, and the Codex variant's absent `interval`).
const DEFAULT_INTERVAL: u64 = 5;

/// How long the Codex variant polls before giving up (auth §10.8). RFC 8628 carries
/// its own `expires_in`; the Codex `usercode` response does not, so the bound is a
/// DECIDED constant — the vendor's own 15 minutes — exactly like `interval`'s 5 s
/// default and `slow_down`'s cumulative 5 s. Not a knob: no flag, no config key.
const CODEX_DEADLINE: u64 = 900;

/// Run the row's device-code flow (auth §7.3). A row with no `device` block has no
/// headless flow at all — a Config error (→78) naming `--browser`, never a silent
/// fallback to the browser flow.
pub(super) fn device_flow(cfg: &OAuthConfig, io: &mut LoginIo) -> Result<Cred, CanonicalError> {
    let device = cfg.device.as_ref().ok_or_else(|| {
        config_err("this provider has no device endpoint; use `--browser`".to_owned())
    })?;
    match device.style {
        DeviceStyle::Rfc8628 => rfc8628_flow(cfg, &device.url, io),
        DeviceStyle::Codex => codex_flow(cfg, &device.url, io),
    }
}

/// Device-code flow (RFC 8628 / auth §7.3): request a device code, print the
/// `user_code` + `verification_uri` to STDERR, then poll the token endpoint every
/// `interval` s (`slow_down` adds 5 s cumulatively) until success, a fatal error,
/// or the `expires_in` deadline (→77). No browser, headless-friendly.
fn rfc8628_flow(
    cfg: &OAuthConfig,
    device_url: &str,
    io: &mut LoginIo,
) -> Result<Cred, CanonicalError> {
    let auth: DeviceAuth = parse_device_json(&collect_body(
        io.transport
            .send(form_post(device_url, &device_params(cfg)))?,
    )?)?;
    prompt(io, &auth.verification_uri, &auth.user_code);

    let deadline = io.clock.now().saturating_add(auth.expires_in);
    poll_until(
        io,
        deadline,
        auth.interval.unwrap_or(DEFAULT_INTERVAL),
        |io| {
            let req = build_token_exchange_request(
                cfg,
                Grant::Device {
                    device_code: &auth.device_code,
                },
            );
            match parse_token_response(&collect_body(io.transport.send(req)?)?, io.clock.now()) {
                Ok(tok) => Ok(Step::Done(tok.as_cred(&Secret::new(""), &None, &None))),
                Err(AuthError::Pending) => Ok(Step::Pending),
                Err(AuthError::SlowDown) => Ok(Step::SlowDown),
                Err(AuthError::Fatal(msg)) => {
                    Err(auth_error(&format!("device login failed: {msg}")))
                }
            }
        },
    )
}

/// Codex's device-code variant (auth §10.8): `POST {base}/deviceauth/usercode` with
/// the `client_id` → `{device_auth_id, user_code, interval}`; the human opens
/// `{base}/codex/device` and types the code; `POST {base}/deviceauth/token` with the
/// pair polls until the provider answers an authorization code plus its PKCE
/// verifier, which the ORDINARY AuthCode exchange then spends against `token_url`.
/// Every URL derives from the row's one `device.url`, and the vendor's own refusal
/// body is streamed verbatim — a workspace with device-code login disabled in its
/// security settings is refused HERE, and brazen quotes the provider rather than
/// compiling a guess about why.
fn codex_flow(cfg: &OAuthConfig, base: &str, io: &mut LoginIo) -> Result<Cred, CanonicalError> {
    let body = serde_json::json!({ "client_id": cfg.client_id }).to_string();
    let (status, bytes) = send(io, json_post(&format!("{base}/deviceauth/usercode"), body))?;
    if !is_success(status) {
        return Err(refused("device authorization", status, &bytes));
    }
    let auth: CodexUserCode = parse_device_json(&bytes)?;
    prompt(io, &format!("{base}/codex/device"), &auth.user_code);

    let deadline = io.clock.now().saturating_add(CODEX_DEADLINE);
    let poll = serde_json::json!({
        "device_auth_id": auth.device_auth_id,
        "user_code": auth.user_code,
    })
    .to_string();
    let url = format!("{base}/deviceauth/token");
    let granted: CodexCode = poll_until(
        io,
        deadline,
        auth.interval.unwrap_or(DEFAULT_INTERVAL),
        |io| {
            let (status, bytes) = send(io, json_post(&url, poll.clone()))?;
            if is_success(status) {
                return Ok(Step::Done(parse_device_json(&bytes)?));
            }
            // The vendor answers 403 while nobody has typed the code yet and 404
            // until the record exists — this variant's `authorization_pending`.
            if status == 403 || status == 404 {
                return Ok(Step::Pending);
            }
            Err(refused("device poll", status, &bytes))
        },
    )?;
    exchange_auth_code(
        cfg,
        io,
        &granted.authorization_code,
        &granted.code_verifier,
        &format!("{base}/deviceauth/callback"),
    )
}

/// What one poll answered: the flow's value, or a reason to go round again.
enum Step<T> {
    Pending,
    SlowDown,
    Done(T),
}

/// The ONE device poll driver both variants run (auth §7.3): stop at the deadline
/// (→77) before polling, pace by the injected `Pacer`, and let `slow_down` add 5 s
/// cumulatively. `step` is the variant's single round-trip; everything about the
/// loop — the deadline check's position, the pacing, the accumulation — is decided
/// here once, so the two wires cannot drift into two different loops.
fn poll_until<T>(
    io: &mut LoginIo,
    deadline: u64,
    mut interval: u64,
    mut step: impl FnMut(&mut LoginIo) -> Result<Step<T>, CanonicalError>,
) -> Result<T, CanonicalError> {
    loop {
        if io.clock.now() >= deadline {
            return Err(auth_error(
                "device login expired before authorization; run `bz --login --provider <id>` again",
            ));
        }
        io.pacer.wait(interval);
        match step(io)? {
            Step::Done(value) => return Ok(value),
            Step::Pending => {}
            Step::SlowDown => interval += DEFAULT_INTERVAL,
        }
    }
}

/// The human prompt, on STDERR (auth §7.3): stdout carries no login payload.
fn prompt(io: &mut LoginIo, verification_uri: &str, user_code: &str) {
    let _ = writeln!(
        io.stderr,
        "To authorize, open {verification_uri} and enter code: {user_code}"
    );
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

/// The Codex `deviceauth/usercode` response (auth §10.8). No `verification_uri` and
/// no `expires_in`: the page and the deadline are the variant's, not the payload's.
#[derive(Deserialize)]
struct CodexUserCode {
    device_auth_id: String,
    user_code: String,
    interval: Option<u64>,
}

/// A successful Codex `deviceauth/token` poll (auth §10.8): an authorization code
/// and the PKCE verifier that redeems it. The response's `code_challenge` is
/// deliberately not read — the exchange needs the verifier alone.
#[derive(Deserialize)]
struct CodexCode {
    authorization_code: String,
    code_verifier: String,
}

fn parse_device_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CanonicalError> {
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

/// A JSON `POST`: the Codex variant's wire shape, beside `form_post`'s RFC 8628 one.
fn json_post(url: &str, body: String) -> WireRequest {
    let mut wire = WireRequest::new(url.to_owned(), body.into_bytes());
    wire.set_header("content-type", "application/json");
    wire
}

/// Send and drain: the STATUS the provider answered plus its body. The Codex variant
/// reads the status (its poll signal is the status code, not an error string), so it
/// carries the value the transport already knows rather than guessing it back from
/// the body — the same discipline `Frame.status` follows on the data plane.
fn send(io: &mut LoginIo, wire: WireRequest) -> Result<(u16, Vec<u8>), CanonicalError> {
    let resp = io.transport.send(wire)?;
    let status = resp.status;
    Ok((status, collect_body(resp)?))
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The provider's own refusal, quoted (auth §10.8). Device-code login can be turned
/// off for a ChatGPT account or workspace in its security settings, and the provider
/// says so in the body — so the body is what the operator reads, verbatim, rather
/// than a sentence brazen invented about a policy it cannot see.
fn refused(stage: &str, status: u16, body: &[u8]) -> CanonicalError {
    let detail = String::from_utf8_lossy(body);
    auth_error(&format!(
        "device login refused by the provider ({stage}, HTTP {status}): {}",
        detail.trim()
    ))
}
