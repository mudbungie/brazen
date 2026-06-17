//! The two error projections of the Anthropic stream (§4.2, §4.3): a whole-body
//! HTTP error keys `kind` off the authoritative status; a mid-stream `error` event
//! on a 2xx stream (no governing status) maps `error.type` through a table. The
//! `error.type` table is the in-band case only; the HTTP case uses the shared
//! `ErrorKind::from_http_status`. `super::decode` dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::json::text_of;

/// A whole-body HTTP error (§4.3): `kind` from the authoritative status via the one
/// shared `ErrorKind::from_http_status`; `error.message`/the `error` object ride
/// `message`/`provider_detail`. The body's `error.type` is a diagnostic only. A body
/// that did not parse (`None` — proxy HTML, empty 5xx) keeps the status-derived kind
/// and degrades to an empty message + `None` detail.
pub(super) fn http_error(body: Option<&Value>, status: u16) -> CanonicalError {
    let err = body.map(|v| &v["error"]);
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: err.map(|e| text_of(e, "message")).unwrap_or_default(),
        provider_detail: err.cloned(),
    }
}

/// Parse a mid-stream `error` event (§4.2): `error.message` → `message`, the full
/// `error` object → `provider_detail`, `error.type` → `kind`. Used ONLY on a 2xx
/// stream, where there is no governing HTTP status to read.
pub(super) fn error_value(v: &Value) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: error_kind(err["type"].as_str().unwrap_or_default()),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

/// Mid-stream `error.type` → `ErrorKind` (§4.2) — the in-band case only, where no
/// HTTP status governs. The HTTP whole-body case uses `from_http_status` instead.
fn error_kind(t: &str) -> ErrorKind {
    use ErrorKind::Provider;
    match t {
        "authentication_error" | "permission_error" => ErrorKind::Auth,
        "invalid_request_error" => Provider { status: 400 },
        "billing_error" => Provider { status: 402 },
        "not_found_error" => Provider { status: 404 },
        "request_too_large" => Provider { status: 413 },
        "rate_limit_error" => Provider { status: 429 },
        "api_error" => Provider { status: 500 },
        "timeout_error" => Provider { status: 504 },
        "overloaded_error" => Provider { status: 529 },
        _ => ErrorKind::Transport, // safe default: retryable, exit 69
    }
}
