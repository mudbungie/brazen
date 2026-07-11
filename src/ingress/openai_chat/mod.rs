//! `openai_chat` ingress: OpenAI `chat/completions` request JSON → `CanonicalRequest`
//! — the exact inverse of the egress `protocol::openai` encoder (openai-chat-mapping
//! §2, read right-to-left). Lossy projections are honest (ingress.md §2): known
//! dialect fields land on the typed canonical fields; unknown TOP-LEVEL keys ride
//! `req.extra` verbatim (forwarded, not rejected — arch §3.1); unknown NESTED keys
//! (`name`, `refusal`, `audio`, …) are ignored, the tolerant-reader stance every
//! decoder in this repo takes toward wire fields it does not know. Structural
//! impossibilities — a shape the canonical model has no slot for — reject with
//! `ParseInput` per the adapt-or-reject ladder's rung 4 (ingress.md §3), naming the
//! offending path; provider POLICY (value ranges, entitlements) is never
//! pre-enforced — carry the spec, not the water (§3). This module holds the shared
//! value-shape getters; `decode` owns the top-level body, `messages` the transcript.
//! The response half — canonical events → `chat.completion(.chunk)` bytes
//! (ingress.md §2, §9, §10) — lives in `encode` (the event fold) and `chunks`
//! (the wire renderings).

mod chunks;
mod decode;
mod encode;
mod messages;

pub(super) use decode::decode_request;
pub(super) use encode::encode_response;

use serde_json::{Map, Value};

use super::IngressError;

/// A rung-4 rejection (ingress.md §3): `ParseInput`, named, before any round-trip.
fn err(message: impl std::fmt::Display) -> IngressError {
    IngressError {
        message: format!("openai_chat ingress: {message}"),
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

/// Optional string: absent and `null` are one absence (OpenAI SDKs emit both).
fn opt_str(v: Option<&Value>, path: &str) -> Result<Option<String>, IngressError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad(path, "a string")),
    }
}

/// Optional bool: absent and `null` are one absence (SDKs emit `strict: null`).
fn opt_bool(v: Option<&Value>, path: &str) -> Result<Option<bool>, IngressError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(path, "a boolean")),
    }
}
