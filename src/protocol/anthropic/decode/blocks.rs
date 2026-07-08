//! The content-block events of the Anthropic stream (§3.4): `content_block_start`
//! opens a tracked block, `content_block_delta` emits a `ContentDelta` (a
//! `signature_delta` → `SignatureDelta`, bl-61a9), and `content_block_stop`
//! closes it. Server-tool blocks SURFACE (CR-4 resolved): `server_tool_use` opens
//! like `tool_use`, and the whole `*_tool_result` family opens by tag SUFFIX with
//! its full `content` inline at start. A block kind with no canonical `ContentKind`
//! is left untracked, so its deltas/stop fall through to `[]`. `super::decode`
//! dispatches into these; the leaf `u32_at`/`text_of` helpers live in
//! `protocol::json`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::json::{text_of, u32_at};
use crate::protocol::{DecodeState, OpenBlock};

/// `content_block_start` → `ContentStart` (§3.4). `server_tool_use` is tracked
/// like `tool_use` (its input arrives as `input_json_delta`s); any other
/// `*_tool_result` tag (except the client `tool_result`, which never opens a
/// stream block) is a server-tool RESULT — tag carried as `kind`, full `content`
/// inline at start, no delta. An unknown kind is left untracked: no event, no
/// `open` entry, so its later deltas/stop fall through to `[]`.
pub(super) fn content_block_start(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = u32_at(v, "index");
    let cb = &v["content_block"];
    let kind = match cb["type"].as_str().unwrap_or_default() {
        "text" => ContentKind::Text {},
        "tool_use" => ContentKind::ToolUse {
            id: text_of(cb, "id"),
            name: text_of(cb, "name"),
        },
        "thinking" => ContentKind::Thinking { id: None }, // Anthropic has no reasoning-item id
        "redacted_thinking" => ContentKind::RedactedThinking {
            data: text_of(cb, "data"), // opaque blob present at block open, carried inline (bl-61a9)
        },
        "server_tool_use" => ContentKind::ServerToolUse {
            id: text_of(cb, "id"),
            name: text_of(cb, "name"),
        },
        t if t.ends_with("_tool_result") && t != "tool_result" => ContentKind::ServerToolResult {
            kind: t.to_owned(),
            tool_use_id: text_of(cb, "tool_use_id"),
            content: cb["content"].clone(),
        },
        _ => return vec![],
    };
    state.open.insert(index, OpenBlock { kind: kind.clone() });
    vec![Event::ContentStart { index, kind }]
}

/// `content_block_delta` → `ContentDelta` (§3.4). A delta for an untracked index
/// emits nothing; `signature_delta` → `SignatureDelta` (bl-61a9).
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
        // signature_delta → SignatureDelta (§3.4, CR-5 resolved bl-61a9): arrives just
        // before the thinking block's stop; a sink folds it onto Thinking.signature.
        "signature_delta" => vec![delta(index, Delta::SignatureDelta(text_of(d, "signature")))],
        _ => vec![],
    }
}

fn delta(index: u32, delta: Delta) -> Event {
    Event::ContentDelta { index, delta }
}

/// `content_block_stop` → `ContentStop` for a tracked block; nothing for an
/// untracked (unknown-kind) one.
pub(super) fn content_block_stop(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let index = u32_at(v, "index");
    if state.open.remove(&index).is_some() {
        vec![Event::ContentStop { index }]
    } else {
        vec![]
    }
}
