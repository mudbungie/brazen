//! RESPONSE projection (anthropic-messages §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s, dispatched on `data.type`. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns the one terminator, §3.8).

use serde_json::Value;

use crate::canonical::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
};
use crate::protocol::{DecodeState, Frame, OpenBlock};

/// Dispatch one frame on its `data.type` (§3.4). A malformed frame surfaces as a
/// Transport error, never a panic; `ping`/unknown keep-alives yield nothing.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let v: Value = serde_json::from_slice(&frame.data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })?;
    // A whole-body error frame carries the HTTP status: its kind comes from the
    // authoritative status (§4.3), not the body's `error.type`. The type-derived
    // path below is only for the mid-stream `error` event on a 2xx stream (§4.2),
    // where no governing status exists.
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&v, status))]);
    }
    Ok(match v["type"].as_str().unwrap_or_default() {
        "message_start" => message_start(&v),
        "content_block_start" => content_block_start(&v, state),
        "content_block_delta" => content_block_delta(&v, state),
        "content_block_stop" => content_block_stop(&v, state),
        "message_delta" => message_delta(&v),
        "message_stop" => {
            state.terminated = true; // native terminator; run appends the one End (§3.8)
            vec![]
        }
        "error" => vec![Event::Error(error_value(&v))], // mid-stream error event (§4.2)
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

/// `content_block_start` → `ContentStart` (§3.4). A block kind with no canonical
/// `ContentKind` (server_tool_use, CR-4) is left untracked: no event, no `open`
/// entry, so its later deltas/stop fall through to `[]`.
fn content_block_start(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    let cb = &v["content_block"];
    let kind = match cb["type"].as_str().unwrap_or_default() {
        "text" => ContentKind::Text {},
        "tool_use" => ContentKind::ToolUse {
            id: text_of(cb, "id"),
            name: text_of(cb, "name"),
        },
        "thinking" => ContentKind::Thinking {},
        "redacted_thinking" => ContentKind::RedactedThinking {},
        _ => return vec![],
    };
    // buffer seeds with redacted_thinking's verbatim `data` (empty otherwise), then
    // accumulates tool-arg json / thinking signature for fold time (§3.2).
    let buffer = text_of(cb, "data");
    state.open.insert(
        index,
        OpenBlock {
            kind: kind.clone(),
            buffer,
        },
    );
    vec![Event::ContentStart { index, kind }]
}

/// `content_block_delta` → `ContentDelta` (or pure state mutation) (§3.4). A delta
/// for an untracked index emits nothing.
fn content_block_delta(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    let Some(block) = state.open.get_mut(&index) else {
        return vec![];
    };
    let d = &v["delta"];
    match d["type"].as_str().unwrap_or_default() {
        "text_delta" => vec![delta(index, Delta::TextDelta(text_of(d, "text")))],
        "input_json_delta" => {
            let frag = text_of(d, "partial_json");
            block.buffer.push_str(&frag); // accumulate; NEVER parse mid-stream
            vec![delta(index, Delta::JsonDelta(frag))]
        }
        "thinking_delta" => vec![delta(index, Delta::ThinkingDelta(text_of(d, "thinking")))],
        "signature_delta" => {
            block.buffer.push_str(&text_of(d, "signature")); // not a Delta (§3.4 / CR-5)
            vec![]
        }
        _ => vec![],
    }
}

fn delta(index: u32, delta: Delta) -> Event {
    Event::ContentDelta { index, delta }
}

/// `content_block_stop` → `ContentStop` for a tracked block; nothing for an
/// untracked one (server_tool_use).
fn content_block_stop(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    if state.open.remove(&index).is_some() {
        vec![Event::ContentStop { index }]
    } else {
        vec![]
    }
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

/// A whole-body HTTP error (§4.3): `kind` from the authoritative status via the one
/// shared `ErrorKind::from_http_status`; `error.message`/the `error` object ride
/// `message`/`provider_detail`. The body's `error.type` is a diagnostic only.
fn http_error(v: &Value, status: u16) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

/// Parse a mid-stream `error` event (§4.2): `error.message` → `message`, the full
/// `error` object → `provider_detail`, `error.type` → `kind`. Used ONLY on a 2xx
/// stream, where there is no governing HTTP status to read.
fn error_value(v: &Value) -> CanonicalError {
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

/// A string field, or `""` when absent/non-string (the wire never panics us).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// The wire `index` (the open-block key); `0` when absent.
fn index(v: &Value) -> u32 {
    v["index"].as_u64().unwrap_or(0) as u32
}
