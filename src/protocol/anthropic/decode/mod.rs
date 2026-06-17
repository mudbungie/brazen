//! RESPONSE projection (anthropic-messages §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s, dispatched on `data.type`. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns the one terminator, §3.8). The content-block
//! events live in [`blocks`], the error envelopes in [`errors`]; this module owns
//! the dispatch and the message-level events.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind, Event, FinishReason, Role, Usage};
use crate::protocol::{DecodeState, Frame};

mod blocks;
mod errors;

/// Dispatch one frame on its `data.type` (§3.4). A malformed frame surfaces as a
/// Transport error, never a panic; `ping`/unknown keep-alives yield nothing.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    // A whole-body error frame carries the HTTP status: its kind comes from the
    // authoritative status (§4.3), regardless of whether the body parses — a proxy
    // 5xx may be HTML or empty. The body is best-effort here, supplying only
    // message/provider_detail. The type-derived path below is the mid-stream `error`
    // event on a 2xx stream (§4.2), where no governing status exists and the body
    // MUST parse to read `error.type`.
    if let Some(status) = frame.status {
        let body = serde_json::from_slice(&frame.data).ok();
        return Ok(vec![Event::Error(errors::http_error(
            body.as_ref(),
            status,
        ))]);
    }
    let v: Value = serde_json::from_slice(&frame.data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })?;
    Ok(match v["type"].as_str().unwrap_or_default() {
        "message_start" => message_start(&v),
        "content_block_start" => blocks::content_block_start(&v, state),
        "content_block_delta" => blocks::content_block_delta(&v, state),
        "content_block_stop" => blocks::content_block_stop(&v, state),
        "message_delta" => message_delta(&v),
        "message_stop" => {
            state.terminated = true; // native terminator; run appends the one End (§3.8)
            vec![]
        }
        "error" => vec![Event::Error(errors::error_value(&v))], // mid-stream error event (§4.2)
        _ => vec![],
    })
}

/// `message_start` → `MessageStart` + (if a usage object is present) `Usage` (§3.4).
fn message_start(v: &Value) -> Vec<Event> {
    let m = &v["message"];
    let mut out = vec![Event::message_start(
        m["id"].as_str().map(str::to_owned),
        m["model"].as_str().map(str::to_owned),
        Role::Assistant, // the wire role is always assistant
    )];
    if let Some(u) = m.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(usage(u)));
    }
    out
}

/// `message_delta` → `Usage?` then `Finish?` (§3.4). `Finish` only when
/// `stop_reason` is non-null (the terminal `message_delta`).
fn message_delta(v: &Value) -> Vec<Event> {
    let mut out = Vec::new();
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(usage(u)));
    }
    let d = &v["delta"];
    if let Some(reason) = d["stop_reason"].as_str() {
        out.push(Event::Finish {
            reason: finish_reason(reason, d),
        });
    }
    out
}

/// `stop_reason` → `FinishReason` (§3.5). Unknown reasons preserve verbatim via
/// `Other` (never a panic).
fn finish_reason(reason: &str, d: &Value) -> FinishReason {
    match reason {
        "end_turn" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "stop_sequence" => FinishReason::StopSequence,
        "tool_use" => FinishReason::ToolUse,
        "pause_turn" => FinishReason::Pause,
        "refusal" => {
            let sd = &d["stop_details"];
            FinishReason::Refusal {
                category: text_of(sd, "category"),
                explanation: sd["explanation"].as_str().map(str::to_owned),
            }
        }
        other => FinishReason::Other(other.to_owned()),
    }
}

/// Anthropic usage object → canonical `Usage` (§3.6): every field `Option`, never
/// a fabricated `0`.
fn usage(u: &Value) -> Usage {
    let field = |k: &str| u[k].as_u64().map(|x| x as u32);
    Usage {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cache_write: field("cache_creation_input_tokens"),
        cache_read: field("cache_read_input_tokens"),
    }
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
pub(super) fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// The wire `index` (the open-block key); `0` when absent.
pub(super) fn index(v: &Value) -> u32 {
    v["index"].as_u64().unwrap_or(0) as u32
}
