//! The `openai_chat` wire renderings of the ingress encoder (ingress.md §2, §4,
//! §9, §10): SSE chunk frames carrying the fabricated-but-well-formed identity,
//! the §4 adaptation comment lines, the `[DONE]` sentinel, the End-rendered
//! aggregate body (the stream, accumulated — §10), the dialect usage object,
//! and the §9 error masquerade. [`super::encode`] decides WHAT each event
//! means; this module owns how it looks on the client wire. serde_json's
//! sorted-key maps make every rendering byte-deterministic for the §14 goldens.

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, Usage};
use crate::ingress::state::IngressState;

/// Emit one delta chunk on the SSE shape; the aggregate shape emits nothing
/// here — the same fold already happened, the body renders at `End` (§10).
pub(super) fn emit(state: &mut IngressState, delta: Value, finish: Option<&str>) -> Vec<u8> {
    if !state.stream {
        return Vec::new();
    }
    let chunk = json!({
        "choices": [{"delta": delta, "finish_reason": finish, "index": 0}],
        "created": state.created,
        "id": state.wire_id(),
        "model": state.wire_model(),
        "object": "chat.completion.chunk",
    });
    let chunk = with_usage_slot(chunk, state);
    frame(state, &chunk)
}

/// When the client asked for usage, every chunk carries the `usage: null` slot
/// the real wire shows until the final usage chunk fills it (§3.4 inverted).
fn with_usage_slot(mut chunk: Value, state: &IngressState) -> Value {
    if state.include_usage {
        chunk["usage"] = Value::Null;
    }
    chunk
}

/// The `include_usage` final chunk (§3.4 inverted): empty `choices`, populated
/// `usage` — emitted after the finish chunk, exactly where real SDKs expect it.
pub(super) fn emit_usage(state: &mut IngressState) -> Vec<u8> {
    if !state.stream {
        return Vec::new();
    }
    let chunk = json!({
        "choices": [],
        "created": state.created,
        "id": state.wire_id(),
        "model": state.wire_model(),
        "object": "chat.completion.chunk",
        "usage": state.usage.clone(),
    });
    frame(state, &chunk)
}

/// The stream terminator: any still-pending adaptation comments (an errored or
/// empty stream may not have emitted a chunk to carry them), then `[DONE]`.
pub(super) fn sentinel(state: &mut IngressState) -> Vec<u8> {
    format!("{}data: [DONE]\n\n", comments(state)).into_bytes()
}

/// Render `payload` as one SSE frame, preceded by any pending §4 adaptation
/// comment lines — SSE-spec-legal, invisible to every conforming parser,
/// visible to `curl` and any debugging eye.
fn frame(state: &mut IngressState, payload: &Value) -> Vec<u8> {
    format!("{}data: {payload}\n\n", comments(state)).into_bytes()
}

/// Drain the pending adaptation names into `: brazen adaptation=<name>` lines.
fn comments(state: &mut IngressState) -> String {
    let mut out = String::new();
    for name in state.pending.drain(..) {
        let _ = writeln!(out, ": brazen adaptation={name}");
    }
    out
}

/// Canonical `Usage` → the dialect's usage object (§3.4 inverted). The wire's
/// slots are required integers once the object exists, so an unreported counter
/// renders 0 here — an owned masquerade fabrication, not a canonical fact
/// (canonically it stays `None`); `total_tokens` is derived, `cached_tokens`
/// appears iff the canonical fact does, and cache writes have no slot.
pub(super) fn usage_json(u: &Usage) -> Value {
    let input = u64::from(u.input_tokens.unwrap_or(0));
    let output = u64::from(u.output_tokens.unwrap_or(0));
    let mut v = json!({
        "completion_tokens": output,
        "prompt_tokens": input,
        "total_tokens": input + output,
    });
    if let Some(c) = u.cache_read_tokens {
        v["prompt_tokens_details"] = json!({"cached_tokens": c});
    }
    v
}

/// The §9 error masquerade. Status: the carried upstream fact when the kind
/// bears one, else the shared `ErrorKind` table read in reverse
/// (`ErrorKind::http_status` — one table, one module). Envelope: this dialect's
/// `{"error":{…}}` with a NUMERIC `code` carrying the status — the proxy
/// convention the forward decoder (§4.3) reads back through the same shared
/// table, so the kind round-trips losslessly. Mid-stream (SSE) the envelope is
/// its own error chunk and the following `End`'s `[DONE]` closes the stream,
/// matching SDK tolerance; on the aggregate shape the envelope becomes the body
/// (rendered at `End`) and `state.status` is the listener's HTTP join point.
pub(super) fn error(e: &CanonicalError, state: &mut IngressState) -> Vec<u8> {
    let status = e.kind.http_status();
    let envelope = json!({"error": {
        "code": status,
        "message": e.message,
        "param": null,
        "type": error_type(status),
    }});
    state.status = Some(status);
    let out = if state.stream {
        frame(state, &envelope)
    } else {
        Vec::new()
    };
    state.error = Some(envelope);
    out
}

/// The dialect's error `type` vocabulary, projected from the status family so
/// client retry logic keeps working (§9): auth, rate-limit, generic 4xx
/// (`ParseInput` lands here → no client retries it), everything else server.
fn error_type(status: u16) -> &'static str {
    match status {
        401 | 403 => "authentication_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    }
}

/// The aggregate body (§10): the stream, accumulated — no second code path. An
/// encoded `Error` event replaces it with the §9 envelope. Either shape carries
/// the §4 exposure: a top-level `"brazen":{"adaptations":[…]}` field when any
/// lossy adaptation fired (typed SDKs drop unknown fields harmlessly).
pub(super) fn body(state: &IngressState) -> Vec<u8> {
    let mut v = match &state.error {
        Some(e) => e.clone(),
        None => success_body(state),
    };
    if !state.adaptations.is_empty() {
        v["brazen"] = json!({"adaptations": state.adaptations});
    }
    v.to_string().into_bytes()
}

/// The folded `chat.completion` (§3, inverted from `decode_full`'s explode):
/// one choice whose `message` is every accumulated fragment made whole.
fn success_body(state: &IngressState) -> Value {
    let mut message = json!({
        "content": or_null(&state.text),
        "refusal": or_null(&state.refusal),
        "role": "assistant",
    });
    if !state.tools.is_empty() {
        message["tool_calls"] = Value::Array(
            state
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "function": {"arguments": t.args, "name": t.name},
                        "id": t.id, "type": "function",
                    })
                })
                .collect(),
        );
    }
    json!({
        "choices": [{"finish_reason": state.finish, "index": 0, "message": message}],
        "created": state.created,
        "id": state.wire_id(),
        "model": state.wire_model(),
        "object": "chat.completion",
        "usage": state.usage,
    })
}

/// The wire's nothing-here spelling: `null`, never `""` (a tool-only turn has
/// `content: null` on the real aggregate).
fn or_null(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        s.into()
    }
}
