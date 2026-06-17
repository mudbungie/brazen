//! The two error projections of the Google stream (§4.8): a whole-body HTTP error
//! keys `kind` off the authoritative status; a mid-stream error chunk on a 2xx
//! stream carries Google's nested `error` whose numeric `code` IS an HTTP status, so
//! `kind` decodes from it through the one shared table (CR-10). `super::decode`
//! dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};

use super::text_of;

/// A whole-body HTTP error (§4.8): Google's nested `{"error":{code,message,status}}`
/// envelope; `kind` comes from the authoritative status, the `error` object rides
/// `message`/`provider_detail`.
pub(super) fn http_error(v: &Value, status: u16) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

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
