//! The envelope (providers §4.10): the ONLY bytes this dialect owns. Request: the
//! `/v1internal:…` path and the `{"model", "request"}` wrapper around `google_genai`'s
//! `body_map`. Response: the `{"response": …}` unwrap in front of `google_genai`'s chunk
//! fold. The client also sends `project`, `requestType`, `requestId`, `userAgent` and a
//! `sessionId`; all were verified unnecessary (auth §11.4), so none is emitted — the
//! empty-set rule, with the door recorded in §9 CR-CC.

use serde_json::{Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest};
use crate::protocol::google_genai::encode::body_map;
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

/// The request path appended to `base_url` (§4.10): no model segment — the model
/// rides the envelope — and the streaming verb picks the endpoint exactly as §4.2.
pub(super) fn request_path(stream: bool) -> &'static str {
    if stream {
        "/v1internal:streamGenerateContent?alt=sse"
    } else {
        "/v1internal:generateContent"
    }
}

/// Build the wire request (§4.10): `{"model": <model>, "request": <§4.2 body>}`. The
/// inner body is `google_genai`'s `body_map` CALLED, not copied, so `extra` /
/// `body_defaults` land inside `request` exactly as on the direct wire.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let url = format!(
        "{}{}",
        ctx.base_url,
        request_path(req.stream.unwrap_or(false))
    );
    let mut envelope = Map::new();
    envelope.insert("model".into(), Value::String(ctx.model.to_owned()));
    envelope.insert("request".into(), Value::Object(body_map(req)?));
    Ok(finish_body(envelope, url))
}

/// The response envelope (§4.10): a chunk (and the non-stream body) is
/// `{"response": <GenerateContentResponse>, "traceId", "metadata"}`. Take `response`
/// when it is an object, else the value itself — so an unwrapped `{"error": …}` body
/// and any future unwrapped chunk take the §4 path unchanged.
pub(super) fn unwrap(v: Value) -> Value {
    match v {
        Value::Object(mut map) if map.get("response").is_some_and(Value::is_object) => {
            map.remove("response").unwrap_or(Value::Null)
        }
        other => other,
    }
}
