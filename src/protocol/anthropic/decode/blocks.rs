//! The content-block events of the Anthropic stream (§3.4): `content_block_start`
//! opens a tracked block, `content_block_delta` emits a `ContentDelta` (a
//! signature delta is not canonical, so it emits nothing), and `content_block_stop`
//! closes it. A block kind with
//! no canonical `ContentKind` is left untracked, so its deltas/stop fall through to
//! `[]`. `super::decode` dispatches into these; the leaf `u32_at`/`text_of` helpers
//! live in `protocol::json`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::json::{text_of, u32_at};
use crate::protocol::{DecodeState, OpenBlock};

/// `content_block_start` → `ContentStart` (§3.4). A block kind with no canonical
/// `ContentKind` (server_tool_use, CR-4) is left untracked: no event, no `open`
/// entry, so its later deltas/stop fall through to `[]`.
pub(super) fn content_block_start(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = u32_at(v, "index");
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
    state.open.insert(index, OpenBlock { kind: kind.clone() });
    vec![Event::ContentStart { index, kind }]
}

/// `content_block_delta` → `ContentDelta` (§3.4). A delta for an untracked index,
/// or a non-canonical `signature_delta`, emits nothing.
pub(super) fn content_block_delta(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = u32_at(v, "index");
    if !state.open.contains_key(&index) {
        return vec![];
    }
    let d = &v["delta"];
    match d["type"].as_str().unwrap_or_default() {
        "text_delta" => vec![delta(index, Delta::TextDelta(text_of(d, "text")))],
        "input_json_delta" => vec![delta(index, Delta::JsonDelta(text_of(d, "partial_json")))],
        "thinking_delta" => vec![delta(index, Delta::ThinkingDelta(text_of(d, "thinking")))],
        // signature_delta is not a canonical Delta (§3.4 / CR-5): the signature
        // rides redacted-thinking semantics, never surfaced as a content delta.
        "signature_delta" => vec![],
        _ => vec![],
    }
}

fn delta(index: u32, delta: Delta) -> Event {
    Event::ContentDelta { index, delta }
}

/// `content_block_stop` → `ContentStop` for a tracked block; nothing for an
/// untracked one (server_tool_use).
pub(super) fn content_block_stop(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = u32_at(v, "index");
    if state.open.remove(&index).is_some() {
        vec![Event::ContentStop { index }]
    } else {
        vec![]
    }
}
