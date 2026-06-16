//! The pure OAuth wire builders + codecs (auth §7.4, §7.5): `build_authorize_url`
//! (PKCE S256), `build_token_exchange_request` (one builder over `Grant`), and
//! `parse_callback` (CSRF), plus the `application/x-www-form-urlencoded` codec they
//! share and the loopback `query_from_request_line` helper. All pure — table-tested
//! from literals (auth §8); the impure socket/browser live in the `bz` bin.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::oauth::{AuthError, Callback, Grant};
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

/// `key=value&…`, every key and value percent-encoded (auth §7.4).
fn encode_pairs(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Split a query into decoded `(key, value)` pairs; a bare `key` (no `=`) yields an
/// empty value, and an empty segment is dropped.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|seg| !seg.is_empty())
        .map(|seg| match seg.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(seg), String::new()),
        })
        .collect()
}

/// RFC 3986 unreserved: never percent-encoded.
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// Percent-encode every non-unreserved byte (form/query value safe).
fn encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// Percent-decode (and `+` → space); a truncated or non-hex `%xx` is left literal.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match hex(bytes[i + 1], bytes[i + 2]) {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            },
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Two hex digits → a byte, or `None` if either is not a hex digit.
fn hex(hi: u8, lo: u8) -> Option<u8> {
    Some((nibble(hi)? << 4) | nibble(lo)?)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
