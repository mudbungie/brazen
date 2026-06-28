//! RESPONSE projection (providers §5.5): one parsed NDJSON line → ≥0 canonical
//! `Event`s. Ollama gives no block structure, so `MessageStart`/`ContentStart` are
//! synthesized; ids are synthesized, and `{"done":true}` is the native terminator.
//! The content-block handlers live in [`blocks`], the error envelopes in [`errors`];
//! this module owns the dispatch, the terminal drain, usage, and helpers. Pure over
//! `(frame, &mut state)`; `decode` never emits `End` (run owns it, §5.5).

use serde_json::Value;

use crate::canonical::{CanonicalError, ContentKind, Event, FinishReason, Role, Usage};
use crate::protocol::json::{http_error, parse};
use crate::protocol::synth::drain;
use crate::protocol::{DecodeState, Frame};

mod blocks;
mod errors;

/// Decode one frame (§5.5): a non-2xx whole-body frame surfaces the raw error body
/// (the shared `http_error`, status-authoritative) — checked BEFORE parsing, so a
/// non-JSON error body keeps its status instead of collapsing to a parse Transport.
/// A mid-stream `{"error":…}` line is an in-band error; anything else is a chat line.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&frame.data, status))]); // §5.9
    }
    let v = parse(&frame.data)?;
    if let Some(err) = v["error"].as_str() {
        return Ok(vec![Event::Error(errors::stream_error(err))]); // mid-stream {"error":…}
    }
    Ok(line(&v, state))
}

/// Decode a COMPLETE non-stream 2xx body (config §4.2). Ollama's `stream:false`
/// returns ONE chat object carrying the whole `message` and `done:true` — exactly
/// the shape `line` already folds: the synthetic stream IS this single line, so
/// replay reduces to one `line` call (the explode is the identity). `MessageStart`,
/// content/tool blocks, drain, usage, and `Finish` all fall out of the existing path.
pub(super) fn decode_full(
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    Ok(line(&parse(body)?, state))
}

/// One chat line → events (§5.5). `MessageStart` fires once; `message.thinking`
/// (the reasoning channel, first), `message.content`, and `message.tool_calls` drive
/// content; `"done":true` drains, reports usage, finishes.
fn line(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let mut out = Vec::new();
    if !state.started {
        state.started = true;
        out.push(Event::message_start(
            None, // Ollama streams no message id (§5.5)
            v["model"].as_str().map(str::to_owned),
            Role::Assistant,
        ));
    }
    let msg = &v["message"];
    blocks::thinking(msg, state, &mut out);
    blocks::text(msg, state, &mut out);
    for call in msg["tool_calls"].as_array().into_iter().flatten() {
        blocks::tool_call(call, state, &mut out);
    }
    if v["done"].as_bool() == Some(true) {
        finish(v, state, &mut out); // the native terminator (§5.5)
    }
    out
}

/// The `done:true` line (§5.5): compute the finish reason (consulting still-open
/// tool blocks), drain every open block to `ContentStop` ascending, then `Usage`,
/// then `Finish`, and flip `terminated`. Order is `… ContentStop* → Usage → Finish`.
fn finish(v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let reason = finish_reason(v, state);
    drain(state, out);
    out.push(Event::Usage(usage(v)));
    out.push(Event::Finish { reason });
    state.terminated = true; // native terminator; run appends the one End (§5.5)
}

/// `done_reason` → `FinishReason` (§5.8). An open tool block promotes to `ToolUse`
/// (Ollama reports `stop` even on a tool call); absent `done_reason` defaults to
/// `Stop`; an unknown string preserves verbatim via `Other` (never a panic).
fn finish_reason(v: &Value, state: &DecodeState) -> FinishReason {
    if state
        .open
        .values()
        .any(|b| matches!(b.kind, ContentKind::ToolUse { .. }))
    {
        return FinishReason::ToolUse;
    }
    match v["done_reason"].as_str() {
        Some("length") => FinishReason::Length,
        None | Some("stop") => FinishReason::Stop,
        Some(other) => FinishReason::Other(other.to_owned()),
    }
}

/// Ollama token stats → canonical `Usage` (§5.7): every field `Option`, never a
/// fabricated `0`. Ollama reports no cache counters → both `None`.
fn usage(v: &Value) -> Usage {
    Usage {
        input_tokens: v["prompt_eval_count"].as_u64().map(|x| x as u32),
        output_tokens: v["eval_count"].as_u64().map(|x| x as u32),
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}
