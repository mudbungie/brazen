//! The content-block handlers of the Google stream (§4.4): a `text` part opens or
//! extends the lazy text block, a `functionCall` part arrives whole — a synthesized
//! `ToolUse` block then a SINGLE `JsonDelta`, left open to close at the terminal
//! drain. `super::decode` dispatches into these; the leaf JSON helpers live in
//! `protocol::json`, the synth mechanics (`next_index`/`open_text`) in `protocol::synth`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::json::{nonempty, text_of, to_json_string};
use crate::protocol::synth::{next_index, open_text};
use crate::protocol::{DecodeState, OpenBlock};

/// One `parts[]` element (§4.4): `text` opens/extends the text block; `functionCall`
/// arrives whole — `ContentStart{ToolUse}` (synth id) then a SINGLE `JsonDelta`,
/// left open to close at the drain.
pub(super) fn part_events(part: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
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
