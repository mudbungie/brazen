//! RESPONSE projection (openai-chat-mapping §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s. Positional `choices[0].delta`; `MessageStart`/`ContentStart`
//! are synthesized (OpenAI gives no block-start), `arguments` stream as `JsonDelta`
//! fragments (never parsed mid-stream), `[DONE]` flips `terminated`, and a non-2xx
//! whole-body frame parses the error envelope. Pure over `(frame, &mut state)`;
//! `decode` never emits `End` (run owns the one terminator, §3.6).

use serde_json::Value;

use crate::canonical::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
};
use crate::protocol::{DecodeState, Frame, OpenBlock};

/// Decode one frame (§3.3): a non-2xx whole-body frame is the error envelope (§4),
/// `[DONE]` is the terminal marker, anything else is a `chat.completion.chunk`.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if frame.whole_body {
        return Ok(vec![Event::Error(error_value(&parse(&frame.data)?))]); // §4
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
    text(delta, state, &mut out);
    if let Some(r) = nonempty(&delta["refusal"]) {
        state.refusal.push_str(r); // not a content block; surfaces at finish (§3.5)
    }
    if let Some(calls) = delta["tool_calls"].as_array() {
        for call in calls {
            tool_call(call, state, &mut out);
        }
    }
    if let Some(reason) = choice["finish_reason"].as_str() {
        finish(reason, state, &mut out); // close every open block, then Finish (§3.3)
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(usage(u))); // emitted after Finish (separate frame, §3.4)
    }
    Ok(out)
}

/// `delta.content` (§3.3): the first non-empty fragment synthesizes the text block
/// (identity before content); each fragment then emits a `TextDelta`. An empty
/// `""` (the role-only chunk, or a stray) opens nothing — avoids an empty block.
fn text(delta: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let Some(t) = nonempty(&delta["content"]) else {
        return;
    };
    let index = match text_index(state) {
        Some(i) => i,
        None => {
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
    };
    out.push(Event::ContentDelta {
        index,
        delta: Delta::TextDelta(t.to_owned()),
    });
}

/// One `delta.tool_calls[]` element (§3.3). First sight of an OpenAI
/// `tool_calls[].index` synthesizes `ContentStart{ToolUse}` (id+name appear only
/// then); later fragments route by that index and emit raw `JsonDelta` — NEVER
/// parsed mid-stream. An empty `arguments` fragment emits nothing (determinism).
fn tool_call(call: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let t = call["index"].as_u64().unwrap_or(0) as u32;
    let index = match state.tool_index.get(&t) {
        Some(&c) => c,
        None => {
            let c = next_index(state);
            let kind = ContentKind::ToolUse {
                id: text_of(call, "id"),
                name: text_of(&call["function"], "name"),
            };
            state.tool_index.insert(t, c);
            state.open.insert(
                c,
                OpenBlock {
                    kind: kind.clone(),
                    buffer: String::new(),
                },
            );
            out.push(Event::ContentStart { index: c, kind });
            c
        }
    };
    if let Some(arg) = nonempty(&call["function"]["arguments"]) {
        if let Some(b) = state.open.get_mut(&index) {
            b.buffer.push_str(arg); // accumulate for fold-time parse; never parsed here
        }
        out.push(Event::ContentDelta {
            index,
            delta: Delta::JsonDelta(arg.to_owned()),
        });
    }
}

/// The finish frame (§3.3): synthesize `ContentStop` for every still-open block in
/// ascending index order (OpenAI sends no per-block stop), then `Finish`.
fn finish(reason: &str, state: &mut DecodeState, out: &mut Vec<Event>) {
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
    out.push(Event::Finish {
        reason: finish_reason(reason, &state.refusal),
    });
}

/// `finish_reason` + accumulated refusal → `FinishReason` (§3.5). A non-empty
/// streamed refusal wins regardless of `finish_reason`; `content_filter` is a
/// refusal with no text; an unknown reason preserves verbatim via `Other`.
fn finish_reason(reason: &str, refusal: &str) -> FinishReason {
    if !refusal.is_empty() {
        return FinishReason::Refusal {
            category: "refusal".into(),
            explanation: Some(refusal.to_owned()),
        };
    }
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolUse,
        "content_filter" => FinishReason::Refusal {
            category: "content_filter".into(),
            explanation: None,
        },
        other => FinishReason::Other(other.to_owned()),
    }
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
/// `error` object → `provider_detail`, the status family (proxied by `error.type`
/// / `error.code`) → `kind`. Emitted as `Event::Error`, never folded into `Finish`.
fn error_value(v: &Value) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: error_kind(text_of(err, "type").as_str(), text_of(err, "code").as_str()),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

/// Error `type`/`code` → `ErrorKind` (§4.2). OpenAI's `type` is not a clean status
/// bijection (a 401 also reads `invalid_request_error`), so the more specific
/// `code` is consulted first; the status rides in `Provider{status}` so exit/
/// retryable derive without a second table.
fn error_kind(ty: &str, code: &str) -> ErrorKind {
    use ErrorKind::Provider;
    match code {
        "invalid_api_key" | "invalid_authentication" => return ErrorKind::Auth,
        "insufficient_quota" | "rate_limit_exceeded" => return Provider { status: 429 },
        _ => {}
    }
    match ty {
        "authentication_error" => ErrorKind::Auth,
        "permission_error" | "permission_denied" => ErrorKind::Auth,
        "invalid_request_error" => Provider { status: 400 },
        "not_found_error" => Provider { status: 404 },
        "rate_limit_error" => Provider { status: 429 },
        "server_error" => Provider { status: 500 },
        "service_unavailable" => Provider { status: 503 },
        _ => ErrorKind::Transport, // safe default: retryable, exit 69
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
fn text_index(state: &DecodeState) -> Option<u32> {
    state
        .open
        .iter()
        .find(|(_, b)| matches!(b.kind, ContentKind::Text {}))
        .map(|(i, _)| *i)
}

/// The next canonical index to assign — computed from the open map (its keys are
/// the dense `0..n` assigned so far), never stored (§3.1; single source of truth).
fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""` so a
/// role-only chunk and a stray empty fragment open no block (§3.3).
fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}
