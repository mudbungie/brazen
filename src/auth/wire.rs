//! The pure OAuth wire builders (auth §7.4, §7.5): `build_authorize_url` (PKCE
//! S256), `build_token_exchange_request` (one builder over `Grant`), and
//! `parse_callback` (CSRF), plus the loopback `query_from_request_line` helper.
//! The `application/x-www-form-urlencoded` codec they share lives in the sibling
//! [`urlencode`](super::urlencode). All pure — table-tested from literals (auth
//! §8); the impure socket/browser live in the `bz` bin.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::oauth::{AuthError, Callback, Grant};
use super::urlencode::{encode_pairs, query_pairs};
use super::OAuthConfig;
use crate::protocol::WireRequest;

/// A PKCE pair (RFC 7636 / auth §7.4): the random `verifier` replayed in the token
/// exchange, and the `challenge` sent in the authorize URL. The verifier is
/// supplied by the control plane (the `bz` bin's RNG); derivation is pure, so it
/// is golden-tested against the RFC 7636 vector.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// Derive S256 (RFC 7636 §4.2): `challenge = BASE64URL-NOPAD(SHA256(verifier))`.
    pub fn derive(verifier: impl Into<String>) -> Pkce {
        let verifier = verifier.into();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

/// The RFC 8252 §4.1 authorization URL with PKCE S256 (auth §7.4): a query of
/// `response_type=code`, `client_id`, `redirect_uri`, `state`, `code_challenge`,
/// `code_challenge_method=S256`, and `scope` only when the row sets it — each value
/// percent-encoded. The exact string is asserted in tests, scope present and absent.
pub fn build_authorize_url(
    cfg: &OAuthConfig,
    pkce: &Pkce,
    state: &str,
    redirect_uri: &str,
) -> String {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", cfg.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("state", state),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    if let Some(scope) = &cfg.scope {
        params.push(("scope", scope.as_str()));
    }
    format!("{}?{}", cfg.authorize_url, encode_pairs(&params))
}

/// The ONE token-exchange builder (auth §7.5): a `POST {token_url}` with a
/// form-encoded body that differs across `Grant` arms ONLY in `grant_type` plus a
/// couple of fields — every arm carries `client_id` and the same content-type, and
/// the same `parse_token_response` reads the way back. Not three code paths; one.
pub fn build_token_exchange_request(cfg: &OAuthConfig, grant: Grant) -> WireRequest {
    let cid = cfg.client_id.as_str();
    let pairs: Vec<(&str, &str)> = match &grant {
        Grant::AuthCode {
            code,
            verifier,
            redirect_uri,
        } => vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("client_id", cid),
        ],
        Grant::Device { device_code } => vec![
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", cid),
        ],
        Grant::Refresh { refresh_token } => vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("client_id", cid),
        ],
    };
    form_post(&cfg.token_url, &pairs)
}

/// A form-encoded `POST`: the shared shape of every OAuth wire request (auth §7.5).
pub(crate) fn form_post(url: &str, pairs: &[(&str, &str)]) -> WireRequest {
    let mut wire = WireRequest::new(url.to_owned(), encode_pairs(pairs).into_bytes());
    wire.set_header("content-type", "application/x-www-form-urlencoded");
    wire
}

/// Validate the loopback callback and extract the code (auth §7.4, §7.5, PURE).
/// An `?error=…` (the user declined) and a missing `code`/`state` are fatal; a
/// returned `state` that does not byte-equal `expected_state` is a CSRF mismatch —
/// fatal, and never proceeding to token exchange (auth §7.4).
pub fn parse_callback(query: &str, expected_state: &str) -> Result<Callback, AuthError> {
    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in query_pairs(query) {
        match key.as_str() {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    if let Some(err) = error {
        return Err(AuthError::Fatal(format!("authorization denied: {err}")));
    }
    let code = code.ok_or_else(|| AuthError::Fatal("callback missing `code`".to_owned()))?;
    let state = state.ok_or_else(|| AuthError::Fatal("callback missing `state`".to_owned()))?;
    if state != expected_state {
        return Err(AuthError::Fatal(
            "callback `state` mismatch (possible CSRF)".to_owned(),
        ));
    }
    Ok(Callback { code, state })
}

/// Extract the query string from an HTTP request line, e.g.
/// `GET /callback?code=x&state=y HTTP/1.1` → `code=x&state=y` (auth §7.4). PURE, so
/// the bin's loopback receiver reads the line over the socket and defers the parse
/// here; `None` when the request-target carries no `?query`.
pub fn query_from_request_line(line: &str) -> Option<String> {
    let target = line.split(' ').nth(1)?;
    target.split_once('?').map(|(_, query)| query.to_owned())
}
