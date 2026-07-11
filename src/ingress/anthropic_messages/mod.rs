//! `anthropic_messages` ingress: Anthropic `POST /v1/messages` request JSON ⇄
//! the canonical model — the exact inverse of the egress `protocol::anthropic`
//! adapter read right-to-left (anthropic-messages spec). `decode` inverts §2's
//! request projection (dialect body → `CanonicalRequest`); `encode` inverts §3's
//! response projection (canonical events → the anthropic-native SSE stream with
//! `message_start`/`content_block_*`/`message_delta`/`message_stop` event framing,
//! plus §4's error envelope). Two anthropic-specific narrowings this dialect
//! discovers (documented, never silent — ingress.md §12):
//!
//! - **The replay stash (§5) is IDLE here.** Anthropic natively carries thinking
//!   `signature`, `redacted_thinking`, and server-tool blocks in-band, so the
//!   encoder emits them as real wire content blocks (never stash writes) and the
//!   decoder reads the client's echoed blocks straight off the request. The wire
//!   `thinking` knob rides `extra` (there is no clean `budget→effort` inverse), so
//!   `req.reasoning` stays `None` and the §5 `thinking_replay` adaptation never
//!   fires — the machinery is reused untouched, simply un-engaged for this dialect.
//! - **The error envelope carries no numeric status.** Anthropic's `{"type":"error",
//!   "error":{"type","message"}}` names only a coarse `error.type` FAMILY, so a
//!   specific upstream status (503) projects to the nearest family (`api_error`) and
//!   re-decodes to that family's canonical status (500). The precise status still
//!   rides the HTTP layer (`IngressState::status`, the listener's status line, §9);
//!   only the in-band `error.type` is coarse (§4.2 read in reverse).
//!
//! Lossy projections are honest (ingress.md §2): known dialect fields land on the
//! typed canonical fields; unknown TOP-LEVEL keys (`thinking`, `metadata`, `top_k`,
//! `service_tier`, `container`, …) ride `req.extra` verbatim; per-block `cache_control`
//! marks are the encoder's own automatic policy (anthropic-messages §2.10) with no
//! canonical home, so the decoder ignores them (the tolerant-reader stance). Structural
//! impossibilities reject with `ParseInput` (rung 4, §3), naming the offending path.

mod acc;
mod decode;
mod encode;
mod frames;
mod messages;

pub(super) use decode::decode_request;
pub(super) use encode::encode_response;
pub(crate) use encode::AnthAcc;

use serde_json::{Map, Value};

use super::IngressError;

/// A rung-4 rejection (ingress.md §3): `ParseInput`, named, before any round-trip.
fn err(message: impl std::fmt::Display) -> IngressError {
    IngressError {
        message: format!("anthropic_messages ingress: {message}"),
    }
}

/// The common wrong-shape rejection: `path` must be `want`.
fn bad(path: &str, want: &str) -> IngressError {
    err(format!("`{path}` must be {want}"))
}

/// Required string at `path` — absence and a wrong type are the same missing fact.
fn str_of<'a>(v: Option<&'a Value>, path: &str) -> Result<&'a str, IngressError> {
    v.and_then(Value::as_str)
        .ok_or_else(|| bad(path, "a string"))
}

/// Required object at `path`.
fn obj_of<'a>(v: Option<&'a Value>, path: &str) -> Result<&'a Map<String, Value>, IngressError> {
    v.and_then(Value::as_object)
        .ok_or_else(|| bad(path, "an object"))
}

/// Required array at `path`.
fn arr_of<'a>(v: Option<&'a Value>, path: &str) -> Result<&'a [Value], IngressError> {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| bad(path, "an array"))
}

/// Optional string: absent and `null` are one absence.
fn opt_str(v: Option<&Value>, path: &str) -> Result<Option<String>, IngressError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad(path, "a string")),
    }
}

/// A `u32` wire field (the token bound); floats and negatives are shapeless here.
fn u32_of(v: &Value, path: &str) -> Result<u32, IngressError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad(path, "an unsigned 32-bit integer"))
}

/// An `f32` wire field (the sampling knobs).
fn f32_of(v: &Value, path: &str) -> Result<f32, IngressError> {
    v.as_f64()
        .map(|f| f as f32)
        .ok_or_else(|| bad(path, "a number"))
}
