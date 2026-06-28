//! The content-block handlers of the Ollama stream (§5.5, §5.6): `message.thinking`
//! opens or extends the lazy thinking block, `message.content` the lazy text block,
//! and each whole `message.tool_calls[]` element synthesizes a `ToolUse` block (synth
//! id) then a SINGLE `JsonDelta`, left open to close at the terminal drain.
//! `super::decode` dispatches into these; the leaf JSON helpers live in
//! `protocol::json`, the synth mechanics (`next_index`/`open_text`/`open_thinking`)
//! in `protocol::synth`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::json::{nonempty, text_of, to_json_string};
use crate::protocol::synth::{next_index, open_text, open_thinking};
use crate::protocol::{DecodeState, OpenBlock};

/// `message.thinking` (§5.5): the reasoning channel surfaced when `think` is enabled.
/// The first non-empty fragment synthesizes the THINKING block (identity before
/// content); each fragment then emits a `ThinkingDelta`, never `TextDelta`.
pub(super) fn thinking(msg: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let Some(t) = nonempty(&msg["thinking"]) else {
        return;
    };
    let index = open_thinking(state, out);
    out.push(Event::ContentDelta {
        index,
        delta: Delta::ThinkingDelta(t.to_owned()),
    });
}

/// `message.content` (§5.5): the first non-empty fragment synthesizes the text
/// block (identity before content); each fragment then emits a `TextDelta`.
pub(super) fn text(msg: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
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
pub(super) fn tool_call(call: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let index = next_index(state);
    let kind = ContentKind::ToolUse {
        id: format!("call_{index}"), // deterministic synth id (§5.6)
        name: text_of(&call["function"], "name"),
    };
    state.open.insert(index, OpenBlock { kind: kind.clone() });
    out.push(Event::ContentStart { index, kind });
    out.push(Event::ContentDelta {
        index,
        delta: Delta::JsonDelta(to_json_string(&call["function"]["arguments"])),
    });
}
