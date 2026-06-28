//! RESPONSE projection (providers §4.4): one parsed SSE frame → ≥0 canonical
//! `Event`s. Each frame is a `GenerateContentResponse` chunk with no per-block
//! start/stop, so `MessageStart`/`ContentStart` are synthesized; the **last chunk's
//! non-null `finishReason`** is the native terminator. The content-block handlers
//! live in [`blocks`], the error envelopes in [`errors`]; this module owns the
//! dispatch, the terminal drain, usage, and helpers. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns it, §4.4).

use serde_json::Value;

use crate::canonical::{CanonicalError, ContentKind, Event, FinishReason, Role, Usage};
use crate::protocol::json::{http_error, parse};
use crate::protocol::synth::drain;
use crate::protocol::{DecodeState, Frame};

mod blocks;
mod errors;

/// Decode one frame (§4.4): a non-2xx whole-body frame surfaces the raw error body
/// (the shared `http_error`, status-authoritative) — checked BEFORE parsing, so a
/// non-JSON error body keeps its status instead of collapsing to a parse Transport.
/// Anything else is a `GenerateContentResponse` chunk.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&frame.data, status))]); // §4.8
    }
    let v = parse(&frame.data)?;
    if v["error"].is_object() {
        return Ok(vec![Event::Error(errors::stream_error(&v["error"]))]); // mid-stream (§4.8)
    }
    Ok(chunk(&v, state))
}

/// Decode a COMPLETE non-stream 2xx body (config §4.2). Google's non-stream
/// `generateContent` returns ONE `GenerateContentResponse` carrying the whole
/// `candidates[0]` (all parts) and a non-null `finishReason` — exactly the shape
/// `chunk` already folds for the terminal chunk: the synthetic stream IS this single
/// response, so replay reduces to one `chunk` call. `MessageStart`, content/tool
/// blocks, drain, usage, and `Finish` all fall out of the existing path.
pub(super) fn decode_full(
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    Ok(chunk(&parse(body)?, state))
}

/// One response chunk → events (§4.4). `MessageStart` fires once; `candidates[0]`
/// parts drive content; a non-null `finishReason` makes this the terminal chunk.
fn chunk(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let mut out = Vec::new();
    if !state.started {
        state.started = true;
        out.push(Event::message_start(
            None, // Google streams no message id (§4.4)
            v["modelVersion"].as_str().map(str::to_owned),
            Role::Assistant,
        ));
    }
    let cand = &v["candidates"][0];
    for part in cand["content"]["parts"].as_array().into_iter().flatten() {
        blocks::part_events(part, state, &mut out);
    }
    match cand["finishReason"].as_str() {
        Some(reason) => finish(reason, v, state, &mut out), // the native terminator (§4.4)
        // A candidate-less chunk carrying `promptFeedback.blockReason` is a prompt-level
        // safety block — terminal, not a truncated stream (§4.4).
        None => match v["promptFeedback"]["blockReason"].as_str() {
            Some(reason) => prompt_block(reason, v, state, &mut out),
            None => {
                if let Some(u) = usage(v) {
                    out.push(Event::Usage(u)); // cumulative usageMetadata, mid-stream
                }
            }
        },
    }
    out
}

/// The `finishReason`-bearing chunk (§4.4): compute the reason (consulting open tool
/// blocks) and run the terminal sequence.
fn finish(reason: &str, v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    terminate(finish_reason(reason, state), v, state, out);
}

/// A prompt-level safety block (§4.4): a candidate-less chunk carrying
/// `promptFeedback.blockReason` is a deterministic refusal of the PROMPT (HTTP 200,
/// exit 0), NOT a truncated stream. It terminates with `Finish{Refusal}` keyed on the
/// block reason — without this it would fall through to a premature-EOF Transport/69.
fn prompt_block(reason: &str, v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    terminate(
        FinishReason::Refusal {
            category: reason.to_lowercase(),
            explanation: None,
        },
        v,
        state,
        out,
    );
}

/// The terminal sequence shared by every Google stop (§4.4): drain every open block
/// to `ContentStop` ascending, then `Usage`, then `Finish`, and flip `terminated`.
/// Order is `… ContentStop* → Usage → Finish`.
fn terminate(reason: FinishReason, v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    drain(state, out);
    if let Some(u) = usage(v) {
        out.push(Event::Usage(u));
    }
    out.push(Event::Finish { reason });
    state.terminated = true; // native terminator; run appends the one End (§4.4)
}

/// `finishReason` → `FinishReason` (§4.7). An open tool block promotes to `ToolUse`
/// (Google reports `STOP` even on a tool call); a safety stop is a `Refusal`
/// (HTTP 200, exit 0); an unknown reason preserves verbatim via `Other`.
fn finish_reason(reason: &str, state: &DecodeState) -> FinishReason {
    if state
        .open
        .values()
        .any(|b| matches!(b.kind, ContentKind::ToolUse { .. }))
    {
        return FinishReason::ToolUse;
    }
    match reason {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" => FinishReason::Refusal {
            category: reason.to_lowercase(),
            explanation: None,
        },
        other => FinishReason::Other(other.to_owned()),
    }
}

/// `usageMetadata` → canonical `Usage` (§4.6), or `None` when absent. Every field
/// `Option`, never a fabricated `0`; Google reports no cache-write.
fn usage(v: &Value) -> Option<Usage> {
    let u = v.get("usageMetadata").filter(|u| u.is_object())?;
    Some(Usage {
        input_tokens: u["promptTokenCount"].as_u64().map(|x| x as u32),
        output_tokens: u["candidatesTokenCount"].as_u64().map(|x| x as u32),
        cache_read_tokens: u["cachedContentTokenCount"].as_u64().map(|x| x as u32),
        cache_write_tokens: None,
    })
}
