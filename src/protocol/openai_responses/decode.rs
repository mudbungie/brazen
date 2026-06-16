//! RESPONSE projection (providers §3.4): one parsed SSE frame → ≥0 canonical
//! `Event`s, dispatched on `data.type`. The Responses stream carries explicit block
//! structure (`output_item`/`content_part` add/done), so the canonical index keys
//! off the wire `output_index` (Anthropic-style — no synthesis). `response.completed`
//! is the native terminator. Pure over `(frame, &mut state)`; `decode` never emits
//! `End` (run owns it, §3.4).

use serde_json::Value;

use crate::canonical::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
};
use crate::protocol::{DecodeState, Frame, OpenBlock};

/// Decode one frame (§3.4): a non-2xx whole-body frame is the OpenAI error envelope
/// (§3.7); anything else is a typed `response.*` event dispatched on `data.type`.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let v = parse(&frame.data)?;
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&v, status))]); // §3.7
    }
    Ok(event(&v, state))
}

/// Dispatch one event on `data.type` (§3.4). Unknown/keep-alive types yield nothing.
fn event(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    match v["type"].as_str().unwrap_or_default() {
        "response.created" | "response.in_progress" => message_start(v, state),
        "response.output_item.added" => item_added(v, state),
        "response.content_part.added" => part_added(v, state),
        "response.output_text.delta" => delta(v, state, Delta::TextDelta),
        "response.function_call_arguments.delta" => delta(v, state, Delta::JsonDelta),
        "response.reasoning_summary_text.delta" => delta(v, state, Delta::ThinkingDelta),
        "response.refusal.delta" => {
            state.refusal.push_str(&text_of(v, "delta")); // surfaces at completion (§3.4)
            vec![]
        }
        "response.output_item.done" => item_done(v, state),
        "response.completed" => completed(v, state),
        "response.incomplete" => incomplete(v, state),
        "response.failed" | "response.error" => vec![Event::Error(stream_error(v))], // §3.7
        _ => vec![],
    }
}

/// `response.created`/`in_progress` → `MessageStart` once, from the `response`
/// object's id+model (gated on `state.started`).
fn message_start(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    if state.started {
        return vec![];
    }
    state.started = true;
    let r = &v["response"];
    vec![Event::message_start(
        r["id"].as_str().map(str::to_owned),
        r["model"].as_str().map(str::to_owned),
        Role::Assistant,
    )]
}

/// `response.output_item.added` (§3.4): a `function_call` item synthesizes
/// `ContentStart{ToolUse}` (identity before content); a `message` item opens lazily
/// on its first content part, so it yields nothing here.
fn item_added(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let item = &v["item"];
    if item["type"].as_str() != Some("function_call") {
        return vec![];
    }
    let index = out_index(v);
    let kind = ContentKind::ToolUse {
        id: text_of(item, "call_id"),
        name: text_of(item, "name"),
    };
    open(state, index, kind.clone());
    vec![Event::ContentStart { index, kind }]
}

/// `response.content_part.added` (§3.4): an `output_text` part synthesizes
/// `ContentStart{Text}` at the item's `output_index`.
fn part_added(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    if v["part"]["type"].as_str() != Some("output_text") {
        return vec![];
    }
    let index = out_index(v);
    open(state, index, ContentKind::Text {});
    vec![Event::ContentStart {
        index,
        kind: ContentKind::Text {},
    }]
}

/// A `*.delta` event → `ContentDelta` at the block's `output_index` (§3.4). A delta
/// for an unopened index emits nothing; the fragment accumulates for fold-time parse
/// and is NEVER parsed mid-stream.
fn delta(v: &Value, state: &mut DecodeState, wrap: fn(String) -> Delta) -> Vec<Event> {
    let index = out_index(v);
    let Some(block) = state.open.get_mut(&index) else {
        return vec![];
    };
    let frag = text_of(v, "delta");
    block.buffer.push_str(&frag);
    vec![Event::ContentDelta {
        index,
        delta: wrap(frag),
    }]
}

/// `response.output_item.done` → `ContentStop` for a tracked block (§3.4); nothing
/// for an untracked one.
fn item_done(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = out_index(v);
    if state.open.remove(&index).is_some() {
        vec![Event::ContentStop { index }]
    } else {
        vec![]
    }
}

/// `response.completed` (§3.4): drain any still-open blocks, emit `Usage` then
/// `Finish`, set `terminated`. A streamed refusal wins; else a `function_call` in
/// the output promotes to `ToolUse`; else the response status (§3.6).
fn completed(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let response = &v["response"];
    let reason = completed_finish(response, &state.refusal);
    terminal(response, reason, state)
}

/// `response.incomplete` (§3.4): `Length` for `max_output_tokens`, else `Other` —
/// then the same drain/usage/finish/terminate as completion.
fn incomplete(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let response = &v["response"];
    let r = response["incomplete_details"]["reason"]
        .as_str()
        .unwrap_or_default();
    let reason = if r == "max_output_tokens" {
        FinishReason::Length
    } else {
        FinishReason::Other(r.to_owned())
    };
    terminal(response, reason, state)
}

/// The shared terminal path (§3.4): drain open blocks → `ContentStop` ascending,
/// then `Usage` (if present), then `Finish`, and flip `terminated`. `decode` never
/// emits `End`; run appends the one terminator at body EOF.
fn terminal(response: &Value, reason: FinishReason, state: &mut DecodeState) -> Vec<Event> {
    let mut out = Vec::new();
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
    if let Some(u) = usage(response) {
        out.push(Event::Usage(u));
    }
    out.push(Event::Finish { reason });
    state.terminated = true;
    out
}

/// `response.completed` status → `FinishReason` (§3.6): a streamed refusal wins; a
/// `function_call` item in the output promotes to `ToolUse`; an unknown status
/// preserves verbatim via `Other`.
fn completed_finish(response: &Value, refusal: &str) -> FinishReason {
    if !refusal.is_empty() {
        return FinishReason::Refusal {
            category: "refusal".into(),
            explanation: Some(refusal.to_owned()),
        };
    }
    let has_tool = response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|i| i["type"].as_str() == Some("function_call"));
    if has_tool {
        return FinishReason::ToolUse;
    }
    match response["status"].as_str() {
        None | Some("completed") => FinishReason::Stop,
        Some(other) => FinishReason::Other(other.to_owned()),
    }
}

/// `response.usage` → canonical `Usage` (§3.5), or `None` when absent. Every field
/// `Option`, never a fabricated `0`; Responses reports no cache-write.
fn usage(response: &Value) -> Option<Usage> {
    let u = response.get("usage").filter(|u| u.is_object())?;
    Some(Usage {
        input: u["input_tokens"].as_u64().map(|x| x as u32),
        output: u["output_tokens"].as_u64().map(|x| x as u32),
        cache_read: u["input_tokens_details"]["cached_tokens"]
            .as_u64()
            .map(|x| x as u32),
        cache_write: None,
    })
}

/// A whole-body HTTP error (§3.7): the OpenAI `{"error":{message,type,…}}` envelope;
/// `kind` from the authoritative status, the `error` object rides `provider_detail`.
fn http_error(v: &Value, status: u16) -> CanonicalError {
    let err = &v["error"];
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
    }
}

/// A mid-stream `response.failed`/`response.error` on a 2xx stream (§3.7): no
/// governing status, so it defaults to `Transport` (retryable, exit 69); the error
/// object rides `provider_detail`. Never folded into `Finish`.
fn stream_error(v: &Value) -> CanonicalError {
    let err = if v["response"]["error"].is_object() {
        v["response"]["error"].clone()
    } else {
        v["error"].clone()
    };
    CanonicalError {
        kind: ErrorKind::Transport,
        message: text_of(&err, "message"),
        provider_detail: Some(err),
    }
}

/// Open a block at `index` with `kind` (the wire-provided `output_index` is the
/// canonical index — Anthropic-style, never synthesized).
fn open(state: &mut DecodeState, index: u32, kind: ContentKind) {
    state.open.insert(
        index,
        OpenBlock {
            kind,
            buffer: String::new(),
        },
    );
}

/// The event's `output_index` — the canonical block index; `0` when absent.
fn out_index(v: &Value) -> u32 {
    v["output_index"].as_u64().unwrap_or(0) as u32
}

/// Parse a frame's bytes as JSON; a malformed frame surfaces as `Transport`.
fn parse(data: &[u8]) -> Result<Value, CanonicalError> {
    serde_json::from_slice(data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
