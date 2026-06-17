//! The content-block events of the Anthropic stream (§3.4): `content_block_start`
//! opens a tracked block, `content_block_delta` emits a `ContentDelta` (or folds a
//! signature into the buffer), and `content_block_stop` closes it. A block kind with
//! no canonical `ContentKind` is left untracked, so its deltas/stop fall through to
//! `[]`. `super::decode` dispatches into these; the shared `index`/`text_of` helpers
//! live in the parent.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::{DecodeState, OpenBlock};

use super::{index, text_of};

/// `content_block_start` → `ContentStart` (§3.4). A block kind with no canonical
/// `ContentKind` (server_tool_use, CR-4) is left untracked: no event, no `open`
/// entry, so its later deltas/stop fall through to `[]`.
pub(super) fn content_block_start(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    let cb = &v["content_block"];
    let kind = match cb["type"].as_str().unwrap_or_default() {
        "text" => ContentKind::Text {},
        "tool_use" => ContentKind::ToolUse {
            id: text_of(cb, "id"),
            name: text_of(cb, "name"),
        },
        "thinking" => ContentKind::Thinking {},
        "redacted_thinking" => ContentKind::RedactedThinking {},
        _ => return vec![],
    };
    // buffer seeds with redacted_thinking's verbatim `data` (empty otherwise), then
    // accumulates tool-arg json / thinking signature for fold time (§3.2).
    let buffer = text_of(cb, "data");
    state.open.insert(
        index,
        OpenBlock {
            kind: kind.clone(),
            buffer,
        },
    );
    vec![Event::ContentStart { index, kind }]
}

/// `content_block_delta` → `ContentDelta` (or pure state mutation) (§3.4). A delta
/// for an untracked index emits nothing.
pub(super) fn content_block_delta(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    let Some(block) = state.open.get_mut(&index) else {
        return vec![];
    };
    let d = &v["delta"];
    match d["type"].as_str().unwrap_or_default() {
        "text_delta" => vec![delta(index, Delta::TextDelta(text_of(d, "text")))],
        "input_json_delta" => {
            let frag = text_of(d, "partial_json");
            block.buffer.push_str(&frag); // accumulate; NEVER parse mid-stream
            vec![delta(index, Delta::JsonDelta(frag))]
        }
        "thinking_delta" => vec![delta(index, Delta::ThinkingDelta(text_of(d, "thinking")))],
        "signature_delta" => {
            block.buffer.push_str(&text_of(d, "signature")); // not a Delta (§3.4 / CR-5)
            vec![]
        }
        _ => vec![],
    }
}

fn delta(index: u32, delta: Delta) -> Event {
    Event::ContentDelta { index, delta }
}

/// `content_block_stop` → `ContentStop` for a tracked block; nothing for an
/// untracked one (server_tool_use).
pub(super) fn content_block_stop(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = index(v);
    if state.open.remove(&index).is_some() {
        vec![Event::ContentStop { index }]
    } else {
        vec![]
    }
}
