//! The two error projections of the Ollama stream (§5.9): a whole-body HTTP error
//! keys `kind` off the authoritative status; a mid-stream `{"error":…}` line on a
//! 2xx stream carries a BARE STRING with no type/code discriminator, so its decoded
//! kind is retryable `Transport` (the honest read of a kindless body, CR-10).
//! `super::decode` dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};

/// A whole-body HTTP error (§5.9): the body is a bare-string envelope
/// `{"error":"…"}`; `kind` comes from the authoritative status, the body rides
/// `message`/`provider_detail`.
pub(super) fn http_error(v: &Value, status: u16) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: v["error"].as_str().unwrap_or_default().to_owned(),
        provider_detail: Some(v.clone()),
    }
}

/// A mid-stream `{"error":…}` line on a 2xx stream (§5.9): `kind` decodes from the
/// body (CR-10, never the transport — a 2xx stream has none), but Ollama's envelope
/// is a BARE STRING with no `type`/`code` discriminator, so the decoded kind is
/// retryable `Transport` (exit 69) — the honest read of a kindless body, not an
/// un-decoded default. Never folded into `Finish`.
pub(super) fn stream_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
    }
}
