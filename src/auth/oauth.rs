//! The pure OAuth core (auth §7.5, §6.1): the credential-bearing types and the
//! parse/predicate functions — `parse_token_response` (ABSOLUTE `expires_at`) and
//! `is_expired`. Zero IO, zero clock beyond the explicit `now` argument, so every
//! branch is table-tested from literals (auth §8). The wire BUILDERS and
//! `parse_callback` live in [`wire`](super::wire); the refresh/apply seam in
//! [`refresh`](super::refresh); the control-plane flows in [`login`](super::login).

use serde::Deserialize;

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
#[derive(Clone, Debug, PartialEq)]
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
}

impl TokenResponse {
    /// Rebuild the stored `Cred::OAuth2` (auth §6.2): the new access token and
    /// absolute `expires_at`, REUSING `prior_refresh` when the response omitted a
    /// rotated refresh token, and carrying `scope` (falling back to the prior).
    pub fn as_cred(&self, prior_refresh: &Secret, prior_scope: &Option<String>) -> Cred {
        Cred::OAuth2 {
            access_token: self.access_token.clone(),
            refresh_token: self
                .refresh_token
                .clone()
                .unwrap_or_else(|| prior_refresh.clone()),
            expires_at: self.expires_at,
            scope: self.scope.clone().or_else(|| prior_scope.clone()),
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
    /// `invalid_grant` / `expired_token` / unknown / malformed / CSRF — fatal (→77).
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
    error: Option<String>,
}

/// Parse a token-endpoint body (auth §7.5). A present `access_token` yields a
/// `TokenResponse` with ABSOLUTE `expires_at = now + expires_in`; otherwise the
/// `error` string maps to the poll signal (`Pending`/`SlowDown`) or a `Fatal`
/// failure. The SAME parser serves refresh, auth-code, and device — they only
/// differ in how they read the result (auth §7.5).
pub fn parse_token_response(bytes: &[u8], now: u64) -> Result<TokenResponse, AuthError> {
    let raw: RawToken = serde_json::from_slice(bytes)
        .map_err(|e| AuthError::Fatal(format!("malformed token response: {e}")))?;
    if let Some(access) = raw.access_token {
        return Ok(TokenResponse {
            access_token: Secret::new(access),
            refresh_token: raw.refresh_token.map(Secret::new),
            expires_at: now + raw.expires_in.unwrap_or(0),
            scope: raw.scope,
        });
    }
    Err(match raw.error.as_deref() {
        Some("authorization_pending") => AuthError::Pending,
        Some("slow_down") => AuthError::SlowDown,
        Some(other) => AuthError::Fatal(other.to_owned()),
        None => AuthError::Fatal("token response had neither access_token nor error".to_owned()),
    })
}
