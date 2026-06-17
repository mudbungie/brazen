//! The content-block + finish events of the OpenAI chat stream (§3.3): `delta.content`
//! synthesizes a lazy text block (OpenAI gives no block-start), `delta.tool_calls[]`
//! synthesizes a `ToolUse` block per OpenAI index and streams `arguments` as raw
//! `JsonDelta` (never parsed mid-stream), and the finish frame drains every open
//! block then emits `Finish`. `super::decode` dispatches into these; the leaf JSON
//! helpers live in `protocol::json`, the synthesized-stream mechanics
//! (`next_index`/`open_text`/`drain`) in `protocol::synth`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event, FinishReason};
use crate::protocol::json::{nonempty, text_of};
use crate::protocol::synth::{drain, next_index, open_text};
use crate::protocol::{DecodeState, OpenBlock};

/// `delta.content` (§3.3): the first non-empty fragment synthesizes the text block
/// (identity before content); each fragment then emits a `TextDelta`. An empty
/// `""` (the role-only chunk, or a stray) opens nothing — avoids an empty block.
pub(super) fn text(delta: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let Some(t) = nonempty(&delta["content"]) else {
        return;
    };
    let index = open_text(state, out);
    out.push(Event::ContentDelta {
        index,
        delta: Delta::TextDelta(t.to_owned()),
    });
}

/// One `delta.tool_calls[]` element (§3.3). First sight of an OpenAI
/// `tool_calls[].index` synthesizes `ContentStart{ToolUse}` (id+name appear only
/// then); later fragments route by that index and emit raw `JsonDelta` — NEVER
/// parsed mid-stream. An empty `arguments` fragment emits nothing (determinism).
pub(super) fn tool_call(call: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
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
pub(super) fn finish(reason: &str, state: &mut DecodeState, out: &mut Vec<Event>) {
    drain(state, out);
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
