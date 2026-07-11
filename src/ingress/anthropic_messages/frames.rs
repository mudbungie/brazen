//! The `anthropic_messages` wire renderings of the ingress encoder (ingress.md §2,
//! §4, §9, §10): the anthropic-native SSE event framing (`event: <name>\ndata:
//! <json>\n\n`, with §4 adaptation comment lines), the `stop_reason` vocabulary, the
//! usage object, the folded non-stream `message` body (the stream, accumulated — §10),
//! and the §9 error envelope. [`super::encode`] decides WHAT each event means; this
//! module owns how it looks on the client wire. serde_json's sorted-key maps make every
//! rendering byte-deterministic for the §14 goldens.

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, FinishReason, Usage};
use crate::ingress::state::IngressState;

/// Render one anthropic SSE event (`event: <name>` + `data: <json>`), preceded by any
/// pending §4 adaptation comment lines — SSE-spec-legal, invisible to conforming
/// parsers, visible to `curl`.
pub(super) fn frame(state: &mut IngressState, name: &str, data: &Value) -> Vec<u8> {
    format!("{}event: {name}\ndata: {data}\n\n", comments(state)).into_bytes()
}

/// Drain the pending adaptation names into `: brazen adaptation=<name>` lines.
fn comments(state: &mut IngressState) -> String {
    let mut out = String::new();
    for name in state.pending.drain(..) {
        let _ = writeln!(out, ": brazen adaptation={name}");
    }
    out
}

/// Response identity (§2): upstream's id when `MessageStart` carried one, else
/// fabricated-but-well-formed from the injected Clock's `created` — the `msg_` prefix
/// every real Anthropic id wears (never openai_chat's `chatcmpl-`).
pub(super) fn wire_id(state: &IngressState) -> String {
    state
        .id
        .clone()
        .unwrap_or_else(|| format!("msg_brazen-{}", state.created))
}

/// The `message_start.message` envelope (§3.4, inverted): `content:[]` and a `usage`
/// snapshot (Anthropic REQUIRES the object; whatever is accumulated so far, `0`
/// before any `Usage` event — an owned masquerade fabrication, the canonical facts
/// stay `None`).
pub(super) fn message_object(state: &IngressState, usage: Value) -> Value {
    json!({
        "type": "message",
        "id": wire_id(state),
        "role": "assistant",
        "model": state.wire_model(),
        "content": [],
        "stop_reason": null,
        "stop_sequence": null,
        "usage": usage,
    })
}

/// Canonical `Usage` → the anthropic usage object (§3.6, inverted): `input_tokens`/
/// `output_tokens` are required integers once the object exists, so an unreported
/// counter renders `0`; the cache fields appear iff the canonical fact does (renamed
/// back to `cache_read_input_tokens`/`cache_creation_input_tokens`).
pub(super) fn usage_json(u: &Usage) -> Value {
    let mut v = json!({
        "input_tokens": u.input_tokens.unwrap_or(0),
        "output_tokens": u.output_tokens.unwrap_or(0),
    });
    if let Some(c) = u.cache_read_tokens {
        v["cache_read_input_tokens"] = json!(c);
    }
    if let Some(c) = u.cache_write_tokens {
        v["cache_creation_input_tokens"] = json!(c);
    }
    v
}

/// `FinishReason` → the wire `stop_reason` + optional `stop_details` (§3.5, inverted).
/// A refusal restates its `category`/`explanation` in `stop_details`; `Other` passes
/// verbatim (never a panic).
pub(super) fn stop_reason(reason: &FinishReason) -> (String, Option<Value>) {
    match reason {
        FinishReason::Stop => ("end_turn".into(), None),
        FinishReason::Length => ("max_tokens".into(), None),
        FinishReason::StopSequence => ("stop_sequence".into(), None),
        FinishReason::ToolUse => ("tool_use".into(), None),
        FinishReason::Pause => ("pause_turn".into(), None),
        FinishReason::Refusal {
            category,
            explanation,
        } => (
            "refusal".into(),
            Some(json!({"category": category, "explanation": explanation})),
        ),
        FinishReason::Other(s) => (s.clone(), None),
    }
}

/// The §9 error masquerade. Status: the carried upstream fact when the kind bears one
/// (`Provider{status}`), else the shared `ErrorKind` table read in reverse
/// (`http_status`). Envelope: this dialect's `{"type":"error","error":{"type",
/// "message"}}` with `type` a COARSE family from the status — Anthropic's envelope has
/// NO numeric status slot, so a specific status re-decodes to its family's status (§4.2
/// read in reverse; the precise status still rides `state.status`, the listener's HTTP
/// line). Returns the `(status, envelope)` the caller stamps onto the state.
pub(super) fn error(e: &CanonicalError) -> (u16, Value) {
    let status = e.kind.http_status();
    let envelope = json!({
        "type": "error",
        "error": {"type": error_type(status), "message": e.message},
    });
    (status, envelope)
}

/// The dialect's error `type` FAMILY, projected from the status so client retry logic
/// keeps working (§9): auth, the named 4xx families, and the 5xx `api_error` bucket
/// (`ParseInput` lands on `invalid_request_error` → no client retries it).
fn error_type(status: u16) -> &'static str {
    match status {
        401 | 403 => "authentication_error",
        400 => "invalid_request_error",
        402 => "billing_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        504 => "timeout_error",
        500..=599 => "api_error",
        _ => "invalid_request_error",
    }
}
