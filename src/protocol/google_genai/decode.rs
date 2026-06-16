//! RESPONSE projection (providers §4.4): one parsed SSE frame → ≥0 canonical
//! `Event`s. Each frame is a `GenerateContentResponse` chunk with no per-block
//! start/stop, so `MessageStart`/`ContentStart` are synthesized; `functionCall`
//! parts arrive WHOLE (one `JsonDelta`, closed at the drain) with a synthesized id;
//! the **last chunk's non-null `finishReason`** is the native terminator. Pure over
//! `(frame, &mut state)`; `decode` never emits `End` (run owns it, §4.4). Open
//! blocks close at the terminal drain — the same monotonic `open.len()` index
//! discipline OpenAI uses (the "never store the index" rule, arch §3.1).

use serde_json::Value;

use crate::canonical::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
};
use crate::protocol::{DecodeState, Frame, OpenBlock};

/// Decode one frame (§4.4): a non-2xx whole-body frame is Google's nested error
/// envelope (§4.8), anything else is a `GenerateContentResponse` chunk.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let v = parse(&frame.data)?;
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&v, status))]); // §4.8
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
        part_events(part, state, &mut out);
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

/// One `parts[]` element (§4.4): `text` opens/extends the text block; `functionCall`
/// arrives whole — `ContentStart{ToolUse}` (synth id) then a SINGLE `JsonDelta`,
/// left open to close at the drain.
fn part_events(part: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    if let Some(t) = nonempty(&part["text"]) {
        let index = open_text(state, out);
        out.push(Event::ContentDelta {
            index,
            delta: Delta::TextDelta(t.to_owned()),
        });
    }
    if let Some(call) = part.get("functionCall").filter(|c| c.is_object()) {
        let index = next_index(state);
        let kind = ContentKind::ToolUse {
            id: format!("call_{index}"), // deterministic synth id (§4.5)
            name: text_of(call, "name"),
        };
        state.open.insert(
            index,
            OpenBlock {
                kind: kind.clone(),
                buffer: String::new(),
            },
        );
        out.push(Event::ContentStart { index, kind });
        out.push(Event::ContentDelta {
            index,
            delta: Delta::JsonDelta(to_json_string(&call["args"])),
        });
    }
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

/// A whole-body HTTP error (§4.8): Google's nested `{"error":{code,message,status}}`
/// envelope; `kind` comes from the authoritative status, the `error` object rides
/// `message`/`provider_detail`.
fn http_error(v: &Value, status: u16) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

/// The canonical index of the open text block, if any; else open one (§4.4).
fn open_text(state: &mut DecodeState, out: &mut Vec<Event>) -> u32 {
    if let Some((i, _)) = state
        .open
        .iter()
        .find(|(_, b)| matches!(b.kind, ContentKind::Text {}))
    {
        return *i;
    }
    let i = next_index(state);
    state.open.insert(
        i,
        OpenBlock {
            kind: ContentKind::Text {},
            buffer: String::new(),
        },
    );
    out.push(Event::ContentStart {
        index: i,
        kind: ContentKind::Text {},
    });
    i
}

/// The next canonical index — the open map's dense `0..n` (blocks never close
/// mid-stream), never stored (arch §3.1).
fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
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

/// A tool-call `args` object → its JSON-encoded string for the single `JsonDelta`
/// fragment (the "valid only concatenated" rule holds trivially, §4.4).
fn to_json_string(args: &Value) -> String {
    #[allow(clippy::expect_used)]
    serde_json::to_string(args).expect("a serde_json::Value re-serializes infallibly")
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""`.
fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
