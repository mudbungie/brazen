//! The mid-stream error projection of the Anthropic stream (§4.2): an `error` event
//! on a 2xx stream (no governing status) maps `error.type` through a table. The
//! whole-body HTTP case (§4.3) lives in the shared `json::http_error` — status is
//! authoritative there — so this `error.type` table is the in-band case only.
//! `super::decode` dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::json::text_of;

/// Parse a mid-stream `error` event (§4.2): `error.message` → `message`, the full
/// `error` object → `provider_detail`, `error.type` → `kind`. Used ONLY on a 2xx
/// stream, where there is no governing HTTP status to read.
pub(super) fn error_value(v: &Value) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: error_kind(err["type"].as_str().unwrap_or_default()),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
        retry_after_seconds: None,
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
