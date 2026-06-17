//! RESPONSE projection (providers §4.4): one parsed SSE frame → ≥0 canonical
//! `Event`s. Each frame is a `GenerateContentResponse` chunk with no per-block
//! start/stop, so `MessageStart`/`ContentStart` are synthesized; the **last chunk's
//! non-null `finishReason`** is the native terminator. The content-block handlers
//! live in [`blocks`], the error envelopes in [`errors`]; this module owns the
//! dispatch, the terminal drain, usage, and helpers. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns it, §4.4).

use serde_json::Value;

use crate::canonical::{CanonicalError, ContentKind, ErrorKind, Event, FinishReason, Role, Usage};
use crate::protocol::{DecodeState, Frame};

mod blocks;
mod errors;

/// Decode one frame (§4.4): a non-2xx whole-body frame is Google's nested error
/// envelope (§4.8), anything else is a `GenerateContentResponse` chunk.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let v = parse(&frame.data)?;
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(errors::http_error(&v, status))]); // §4.8
    }
    if v["error"].is_object() {
        return Ok(vec![Event::Error(errors::stream_error(&v["error"]))]); // mid-stream (§4.8)
    }
    Ok(chunk(&v, state))
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
        None => {
            if let Some(u) = usage(v) {
                out.push(Event::Usage(u)); // cumulative usageMetadata, mid-stream
            }
        }
    }
    out
}

/// The `finishReason`-bearing chunk (§4.4): compute the reason (consulting open
/// tool blocks), drain every open block to `ContentStop` ascending, then `Usage`,
/// then `Finish`, and flip `terminated`. Order is `… ContentStop* → Usage → Finish`.
fn finish(reason: &str, v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let finish = finish_reason(reason, state);
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
    if let Some(u) = usage(v) {
        out.push(Event::Usage(u));
    }
    out.push(Event::Finish { reason: finish });
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
        input: u["promptTokenCount"].as_u64().map(|x| x as u32),
        output: u["candidatesTokenCount"].as_u64().map(|x| x as u32),
        cache_read: u["cachedContentTokenCount"].as_u64().map(|x| x as u32),
        cache_write: None,
    })
}

/// Parse a frame's bytes as JSON; a malformed chunk surfaces as `Transport`, never
/// a panic.
fn parse(data: &[u8]) -> Result<Value, CanonicalError> {
    serde_json::from_slice(data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })
}

/// The next canonical index — the open map's dense `0..n` (blocks never close
/// mid-stream), never stored (arch §3.1).
pub(super) fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
}

/// A tool-call `args` object → its JSON-encoded string for the single `JsonDelta`
/// fragment (the "valid only concatenated" rule holds trivially, §4.4).
pub(super) fn to_json_string(args: &Value) -> String {
    #[allow(clippy::expect_used)]
    serde_json::to_string(args).expect("a serde_json::Value re-serializes infallibly")
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
pub(super) fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""`.
pub(super) fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
