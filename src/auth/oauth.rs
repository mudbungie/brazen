//! The pure OAuth core (auth §7.5, §6.1): the credential-bearing types and the
//! parse/predicate functions — `parse_token_response` (ABSOLUTE `expires_at`) and
//! `is_expired`. Zero IO, zero clock beyond the explicit `now` argument, so every
//! branch is table-tested from literals (auth §8). The wire BUILDERS and
//! `parse_callback` live in [`wire`](super::wire); the refresh/apply seam in
//! [`refresh`](super::refresh); the control-plane flows in [`login`](super::login).

use serde::Deserialize;

use crate::jwt::{jwt_account_id, jwt_exp};
use crate::store::{Cred, Secret};

/// Seconds of clock-skew / in-flight margin (auth §6.1): refresh slightly early so
/// a token cannot expire between `apply` and the provider receiving the request.
pub const SKEW: u64 = 60;

/// Freshness is a QUERY, never a stored `is_valid` flag (auth §6.1): a token is
/// stale once `now + SKEW` reaches its absolute `expires_at`. The boundary
/// `now == expires_at - SKEW` is stale (the `>=`).
pub fn is_expired(expires_at: u64, now: u64) -> bool {
    now + SKEW >= expires_at
}

/// The `?code=&state=` a successful authorization redirect carries (auth §7.2),
/// extracted and CSRF-checked by [`parse_callback`](super::wire::parse_callback).
// No `Debug`: the `code` field is the raw OAuth authorization code — a bearer
// secret — and a derived `Debug` would print it (the siblings `Pkce`/`Grant`
// omit it for the same reason, auth §7.5).
#[derive(Clone, PartialEq)]
pub struct Callback {
    pub code: String,
    pub state: String,
}

/// The unifying token-exchange input (auth §7.5): auth-code, device-poll, and
/// silent refresh are ONE `POST {token_url}` differing only in form-body params,
/// so [`build_token_exchange_request`](super::wire::build_token_exchange_request)
/// matches on this to fill the body and is otherwise identical across all three.
pub enum Grant<'a> {
    AuthCode {
        code: &'a str,
        verifier: &'a str,
        redirect_uri: &'a str,
        /// The sign-in's CSRF `state`, when the flow that obtained the code holds
        /// one. Sent only by a `token_style = "json"` row (auth §7.5).
        state: Option<&'a str>,
    },
    Device {
        device_code: &'a str,
    },
    Refresh {
        refresh_token: &'a Secret,
    },
}

/// A parsed SUCCESSFUL token response (auth §7.5). `expires_at` is ABSOLUTE —
/// computed ONCE as `now + expires_in` at parse time (auth §5.1) — so it is never
/// the relative value, read back wrong in a later process.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenResponse {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub expires_at: u64,
    pub scope: Option<String>,
    /// The `account_id` derived from a returned id_token (auth §10.4); `None` when
    /// the response carried no id_token (a refresh response usually omits it).
    pub account_id: Option<String>,
}

impl TokenResponse {
    /// Rebuild the stored `Cred::OAuth2` (auth §6.2): the new access token and
    /// absolute `expires_at`, REUSING `prior_refresh`/`prior_scope`/`prior_account_id`
    /// when the response omitted a rotated/fresh value, and carrying the rest.
    pub fn as_cred(
        &self,
        prior_refresh: &Secret,
        prior_scope: &Option<String>,
        prior_account_id: &Option<String>,
    ) -> Cred {
        Cred::OAuth2 {
            access_token: self.access_token.clone(),
            refresh_token: self
                .refresh_token
                .clone()
                .unwrap_or_else(|| prior_refresh.clone()),
            expires_at: self.expires_at,
            scope: self.scope.clone().or_else(|| prior_scope.clone()),
            account_id: self.account_id.clone().or_else(|| prior_account_id.clone()),
        }
    }
}

/// How a token-endpoint error body is interpreted (auth §7.5). The SAME parser
/// returns these; callers differ in interpretation — the device poll reads
/// `Pending`/`SlowDown` as CONTINUE, while refresh and auth-code read any error as
/// FATAL → 77. CSRF mismatch and a malformed body are also `Fatal`.
#[derive(Clone, Debug, PartialEq)]
pub enum AuthError {
    /// `authorization_pending` — device flow keeps polling.
    Pending,
    /// `slow_down` — device flow adds 5 s to the interval, then keeps polling.
    SlowDown,
    /// `invalid_grant` / `expired_token` / unknown / unreadable / CSRF — fatal (→77).
    /// The string is the vendor's own words wherever it gave any.
    Fatal(String),
}

/// The success-or-error shape of a token-endpoint body (auth §7.5): a present
/// `access_token` is success; otherwise `error` names the poll signal / failure.
#[derive(Deserialize)]
struct RawToken {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    /// The OpenID Connect id_token (auth §10.4): present in a login response,
    /// carrying the `account_id` claim; usually omitted on refresh.
    id_token: Option<String>,
    error: Option<RawError>,
    /// RFC 6749 §5.2's human-readable sentence beside a string `error`.
    error_description: Option<String>,
}

/// The two shapes a token endpoint's `error` arrives in (auth §7.5): RFC 6749's
/// string code, or an object carrying its own `type` and `message`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawError {
    Code(String),
    Object {
        #[serde(rename = "type")]
        kind: Option<String>,
        message: Option<String>,
    },
}

/// The most of a vendor's text one error line quotes, in characters.
const QUOTE: usize = 300;

/// Cut `text` to [`QUOTE`] characters, marking the cut.
fn clip(text: &str) -> String {
    match text.char_indices().nth(QUOTE) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// A body the parser could not read, in the vendor's own bytes (auth §7.5) — unless
/// it names a token field: a body that may hold a credential is never printed.
fn unreadable(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let keys = ["\"access_token\"", "\"refresh_token\"", "\"id_token\""];
    if keys.iter().any(|key| text.contains(key)) {
        return "unreadable token response (it carries a token, so it is not shown)".to_owned();
    }
    format!("unreadable token response: {}", clip(text.trim()))
}

/// Parse a token-endpoint body (auth §7.5). A present `access_token` yields a
/// `TokenResponse` with ABSOLUTE `expires_at = now + expires_in`; otherwise the
/// `error` — a string code or an object's `type` — maps to the poll signal
/// (`Pending`/`SlowDown`) or a `Fatal` carrying the vendor's code and sentence, and
/// a body with neither is quoted, bounded ([`unreadable`]). The SAME parser serves
/// refresh, auth-code, and device — they only differ in how they read the result.
pub fn parse_token_response(bytes: &[u8], now: u64) -> Result<TokenResponse, AuthError> {
    let Ok(raw) = serde_json::from_slice::<RawToken>(bytes) else {
        return Err(AuthError::Fatal(unreadable(bytes)));
    };
    if let Some(access) = raw.access_token {
        // Expiry source is single-pathed with an empty case (auth §10.3):
        // `expires_in` (Anthropic) ⇒ relative→absolute; absent ⇒ the access token's
        // own absolute JWT `exp` (OpenAI sends no `expires_in`); neither ⇒ `now`
        // (immediately stale, forcing one refresh, never a fixed bogus instant).
        let expires_at = match raw.expires_in {
            Some(secs) => now.saturating_add(secs),
            None => jwt_exp(&access).unwrap_or(now),
        };
        let account_id = raw.id_token.as_deref().and_then(jwt_account_id);
        return Ok(TokenResponse {
            access_token: Secret::new(access),
            refresh_token: raw.refresh_token.map(Secret::new),
            expires_at,
            scope: raw.scope,
            account_id,
        });
    }
    let (code, detail) = match raw.error {
        Some(RawError::Code(code)) => (Some(code), raw.error_description),
        Some(RawError::Object { kind, message }) => (kind, message),
        None => (None, None),
    };
    Err(match (code.as_deref(), detail) {
        (Some("authorization_pending"), _) => AuthError::Pending,
        (Some("slow_down"), _) => AuthError::SlowDown,
        (Some(code), Some(detail)) => AuthError::Fatal(clip(&format!("{code}: {detail}"))),
        (Some(code), None) => AuthError::Fatal(clip(code)),
        (None, Some(detail)) => AuthError::Fatal(clip(&detail)),
        (None, None) => AuthError::Fatal(unreadable(bytes)),
    })
}
