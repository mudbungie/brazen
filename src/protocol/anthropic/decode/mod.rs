//! RESPONSE projection (anthropic-messages §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s, dispatched on `data.type`. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns the one terminator, §3.8). The content-block
//! events live in [`blocks`], the error envelopes in [`errors`]; this module owns
//! the dispatch and the message-level events.

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, Event, FinishReason, Role, Usage};
use crate::protocol::json::{http_error, parse, text_of, to_json_string};
use crate::protocol::{DecodeState, Frame};

mod blocks;
mod errors;

/// Decode a COMPLETE non-stream 2xx body (config §4.2). A `stream:false` Messages
/// body is the WHOLE `message` object — the same one `message_start` wraps, plus a
/// final `content[]` and a top-level `stop_reason`/`usage`. The explode→replay
/// reconstructs the stream's synthetic frames and drives them through the SAME
/// dispatch the stream uses: `message_start` (id/model/usage), then each `content[i]`
/// block fanned to its `content_block_start`→`content_block_delta`→`content_block_stop`
/// triplet (text/tool/thinking/redacted each in the wire delta shape `blocks` already
/// folds, the array position the wire index), then `message_delta` (stop_reason +
/// `stop_details` → `Finish`). One Usage, the full final counts (§3.6, last-wins).
pub(super) fn decode_full(
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    let v = parse(body)?;
    let mut out = message_start(&json!({ "message": v }));
    for (index, block) in v["content"].as_array().into_iter().flatten().enumerate() {
        explode_block(index as u32, block, state, &mut out);
    }
    out.extend(message_delta(&json!({ "delta": {
        "stop_reason": v["stop_reason"],
        "stop_sequence": v["stop_sequence"],
        "stop_details": v["stop_details"],
    }})));
    Ok(out)
}

/// One finished `content[i]` block → its synthetic start/delta/stop triplet, driven
/// through the SAME `blocks` handlers the stream uses (§3.4). `text`/`thinking` carry
/// one text/thinking delta; `tool_use` AND `server_tool_use` re-serialize the WHOLE
/// `input` as a single `input_json_delta`; `redacted_thinking` and the `*_tool_result`
/// family open and emit no delta (a result's `content` already rode its start) —
/// each the exact wire delta shape `content_block_delta` already handles.
fn explode_block(index: u32, block: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let start = json!({ "index": index, "content_block": block });
    out.extend(blocks::content_block_start(&start, state));
    let delta = match block["type"].as_str().unwrap_or_default() {
        "text" => json!({ "type": "text_delta", "text": block["text"] }),
        "tool_use" | "server_tool_use" => {
            json!({ "type": "input_json_delta", "partial_json": to_json_string(&block["input"]) })
        }
        "thinking" => json!({ "type": "thinking_delta", "thinking": block["thinking"] }),
        _ => Value::Null, // redacted_thinking / *_tool_result open whole, stream no delta
    };
    if !delta.is_null() {
        out.extend(blocks::content_block_delta(
            &json!({ "index": index, "delta": delta }),
            state,
        ));
    }
    out.extend(blocks::content_block_stop(
        &json!({ "index": index }),
        state,
    ));
}

/// Dispatch one frame on its `data.type` (§3.4). A malformed frame surfaces as a
/// Transport error, never a panic; `ping`/unknown keep-alives yield nothing.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    // A whole-body error frame carries the HTTP status: its kind comes from the
    // authoritative status (§4.3), regardless of whether the body parses — a proxy
    // 5xx may be HTML or empty. The raw body rides provider_detail verbatim (shared
    // `http_error`). The type-derived path below is the mid-stream `error` event on
    // a 2xx stream (§4.2), where no governing status exists and the body MUST parse
    // to read `error.type`.
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&frame.data, status))]);
    }
    let v: Value = parse(&frame.data)?;
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
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_write_tokens: field("cache_creation_input_tokens"),
        cache_read_tokens: field("cache_read_input_tokens"),
    }
}
