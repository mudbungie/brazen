//! The mid-stream error projection of the Google stream (§4.8): an error chunk on a
//! 2xx stream carries Google's nested `error` whose numeric `code` IS an HTTP status,
//! so `kind` decodes from it through the one shared table (CR-10). The whole-body
//! HTTP case lives in the shared `json::http_error` — status is authoritative there.
//! `super::decode` dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::json::text_of;

/// A mid-stream error chunk on a 2xx SSE stream (§4.8): the body carries Google's
/// nested `error` object whose numeric `code` IS an HTTP-status int, so `kind`
/// decodes from it through the one shared table (CR-10 — the body's code, never the
/// transport status, which a 2xx stream lacks). The `error` rides `provider_detail`.
/// Never folded into `Finish`.
pub(super) fn stream_error(err: &Value) -> CanonicalError {
    let code = err["code"].as_u64().unwrap_or(0) as u16;
    CanonicalError {
        kind: ErrorKind::from_http_status(code),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}
