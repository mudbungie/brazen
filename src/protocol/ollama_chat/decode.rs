//! RESPONSE projection (providers §5.5): one parsed NDJSON line → ≥0 canonical
//! `Event`s. Ollama gives no block structure, so `MessageStart`/`ContentStart` are
//! synthesized; tool calls arrive WHOLE (one `JsonDelta` then closed at the drain),
//! ids are synthesized, and `{"done":true}` is the native terminator. Pure over
//! `(frame, &mut state)`; `decode` never emits `End` (run owns the one terminator,
//! §5.5). Open blocks close at the terminal drain — the same single drain point and
//! monotonic `open.len()` index discipline OpenAI uses (the "never store the index"
//! rule, arch §3.1).

use serde_json::Value;

use crate::canonical::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
};
use crate::protocol::{DecodeState, Frame, OpenBlock};

/// Decode one frame (§5.5): a non-2xx whole-body frame is the bare-string error
/// envelope (§5.9), a mid-stream `{"error":…}` line is an in-band error, anything
/// else is a chat line.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let v = parse(&frame.data)?;
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&v, status))]); // §5.9
    }
    if let Some(err) = v["error"].as_str() {
        return Ok(vec![Event::Error(stream_error(err))]); // mid-stream {"error":…}
    }
    Ok(line(&v, state))
}

/// One chat line → events (§5.5). `MessageStart` fires once; `message.content` and
/// `message.tool_calls` drive content; `"done":true` drains, reports usage, finishes.
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
    text(msg, state, &mut out);
    for call in msg["tool_calls"].as_array().into_iter().flatten() {
        tool_call(call, state, &mut out);
    }
    if v["done"].as_bool() == Some(true) {
        finish(v, state, &mut out); // the native terminator (§5.5)
    }
    out
}

/// `message.content` (§5.5): the first non-empty fragment synthesizes the text
/// block (identity before content); each fragment then emits a `TextDelta`.
fn text(msg: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let Some(t) = nonempty(&msg["content"]) else {
        return;
    };
    let index = open_text(state, out);
    out.push(Event::ContentDelta {
        index,
        delta: Delta::TextDelta(t.to_owned()),
    });
}

/// One whole `message.tool_calls[]` element (§5.6): synthesize `ContentStart{ToolUse}`
/// (id synthesized — Ollama sends none), then the complete `arguments` object as a
/// SINGLE `JsonDelta`. The block stays open and closes at the terminal drain.
fn tool_call(call: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let index = next_index(state);
    let kind = ContentKind::ToolUse {
        id: format!("call_{index}"), // deterministic synth id (§5.6)
        name: text_of(&call["function"], "name"),
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
        delta: Delta::JsonDelta(to_json_string(&call["function"]["arguments"])),
    });
}

/// The `done:true` line (§5.5): compute the finish reason (consulting still-open
/// tool blocks), drain every open block to `ContentStop` ascending, then `Usage`,
/// then `Finish`, and flip `terminated`. Order is `… ContentStop* → Usage → Finish`.
fn finish(v: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let reason = finish_reason(v, state);
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
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
        input: v["prompt_eval_count"].as_u64().map(|x| x as u32),
        output: v["eval_count"].as_u64().map(|x| x as u32),
        cache_read: None,
        cache_write: None,
    }
}

/// A whole-body HTTP error (§5.9): the body is a bare-string envelope
/// `{"error":"…"}`; `kind` comes from the authoritative status, the body rides
/// `message`/`provider_detail`.
fn http_error(v: &Value, status: u16) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: v["error"].as_str().unwrap_or_default().to_owned(),
        provider_detail: Some(v.clone()),
    }
}

/// A mid-stream `{"error":…}` line on a 2xx stream (§5.9): `kind` decodes from the
/// body (CR-10, never the transport — a 2xx stream has none), but Ollama's envelope
/// is a BARE STRING with no `type`/`code` discriminator, so the decoded kind is
/// retryable `Transport` (exit 69) — the honest read of a kindless body, not an
/// un-decoded default. Never folded into `Finish`.
fn stream_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// The canonical index of the open text block, if any; else open one (§5.5).
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

/// The next canonical index — computed from the open map (its keys are the dense
/// `0..n` assigned so far; blocks never close mid-stream), never stored (arch §3.1).
fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
}

/// Parse a frame's bytes as JSON; a malformed line surfaces as `Transport`, never a
/// panic (the wire never crashes us).
fn parse(data: &[u8]) -> Result<Value, CanonicalError> {
    serde_json::from_slice(data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })
}

/// A tool-call `arguments` `Value` → its JSON-encoded string for the single
/// `JsonDelta` fragment (the "valid only concatenated" rule holds trivially, §5.6).
fn to_json_string(input: &Value) -> String {
    #[allow(clippy::expect_used)]
    serde_json::to_string(input).expect("a serde_json::Value re-serializes infallibly")
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""`.
fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
