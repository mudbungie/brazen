//! RESPONSE projection (openai-chat-mapping §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s. Positional `choices[0].delta`; `MessageStart`/`ContentStart`
//! are synthesized (OpenAI gives no block-start), `arguments` stream as `JsonDelta`
//! fragments (never parsed mid-stream), `[DONE]` flips `terminated`, and a non-2xx
//! whole-body frame parses the error envelope. The content-block + finish events
//! live in [`blocks`]; this module owns the dispatch, usage, error, and helpers.
//! Pure over `(frame, &mut state)`; `decode` never emits `End` (run owns it, §3.6).

use serde_json::Value;

use crate::canonical::{CanonicalError, ContentKind, ErrorKind, Event, Role, Usage};
use crate::protocol::{DecodeState, Frame};

mod blocks;

/// Decode one frame (§3.3): a non-2xx whole-body frame is the error envelope (§4),
/// `[DONE]` is the terminal marker, anything else is a `chat.completion.chunk`.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if let Some(status) = frame.status {
        // The status is authoritative (§4): the kind derives from it even when the
        // body does not parse (proxy HTML, empty 5xx). The body is best-effort,
        // supplying only message/provider_detail.
        let body = parse(&frame.data).ok();
        return Ok(vec![Event::Error(error_value(body.as_ref(), status))]); // §4
    }
    if frame.data == b"[DONE]" {
        state.terminated = true; // provider terminal marker; run appends the one End (§3.6)
        return Ok(vec![]);
    }
    chunk(&parse(&frame.data)?, state)
}

/// One `chat.completion.chunk` → events (§3.3). MessageStart fires once on the
/// first chunk; then `choices[0].delta` drives text / tool / refusal / finish, and
/// a populated top-level `usage` (the separate later chunk) yields `Usage`.
fn chunk(v: &Value, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let mut out = Vec::new();
    if !state.started {
        state.started = true;
        out.push(Event::message_start(
            v["id"].as_str().map(str::to_owned),
            v["model"].as_str().map(str::to_owned),
            Role::Assistant,
        ));
    }
    let choice = &v["choices"][0];
    let delta = &choice["delta"];
    blocks::text(delta, state, &mut out);
    if let Some(r) = nonempty(&delta["refusal"]) {
        state.refusal.push_str(r); // not a content block; surfaces at finish (§3.5)
    }
    if let Some(calls) = delta["tool_calls"].as_array() {
        for call in calls {
            blocks::tool_call(call, state, &mut out);
        }
    }
    if let Some(reason) = choice["finish_reason"].as_str() {
        blocks::finish(reason, state, &mut out); // close every open block, then Finish (§3.3)
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(usage(u))); // emitted after Finish (separate frame, §3.4)
    }
    Ok(out)
}

/// OpenAI `usage` → canonical `Usage` (§3.4): every field `Option`, never a
/// fabricated `0`. `cached_tokens` is `Some` iff `prompt_tokens_details` carried
/// it (absent → `None`, present `0` → `Some(0)`); `cache_write` has no equivalent.
fn usage(u: &Value) -> Usage {
    Usage {
        input: u["prompt_tokens"].as_u64().map(|x| x as u32),
        output: u["completion_tokens"].as_u64().map(|x| x as u32),
        cache_read: u["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .map(|x| x as u32),
        cache_write: None,
    }
}

/// Parse the OpenAI error envelope (§4.1): `error.message` → `message`, the whole
/// `error` object → `provider_detail`, and the **HTTP status** → `kind` via the one
/// shared `ErrorKind::from_http_status` table. The body's `error.type`/`error.code`
/// are diagnostics that ride `provider_detail` verbatim — never consulted for the
/// kind (the status is the authoritative fact). Emitted as `Event::Error`, never
/// folded into `Finish`. A body that did not parse (`None` — proxy HTML, empty 5xx)
/// keeps the status-derived kind and degrades to an empty message + `None` detail.
fn error_value(body: Option<&Value>, status: u16) -> CanonicalError {
    let err = body.map(|v| &v["error"]);
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: err.map(|e| text_of(e, "message")).unwrap_or_default(),
        provider_detail: err.cloned(),
    }
}

/// Parse a frame's bytes as JSON; a malformed body surfaces as a `Transport`
/// error, never a panic (the wire never crashes us).
fn parse(data: &[u8]) -> Result<Value, CanonicalError> {
    serde_json::from_slice(data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })
}

/// The canonical index of the open text block, if any (at most one).
pub(super) fn text_index(state: &DecodeState) -> Option<u32> {
    state
        .open
        .iter()
        .find(|(_, b)| matches!(b.kind, ContentKind::Text {}))
        .map(|(i, _)| *i)
}

/// The next canonical index to assign — computed from the open map (its keys are
/// the dense `0..n` assigned so far), never stored (§3.1; single source of truth).
pub(super) fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
pub(super) fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""` so a
/// role-only chunk and a stray empty fragment open no block (§3.3).
pub(super) fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
