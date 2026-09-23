//! The content-block handlers of the Google stream (§4.4): a `text` part opens or
//! extends the lazy text block — or, when flagged `thought: true`, the lazy THINKING
//! block (`ThinkingDelta`, never the answer text) — and a `functionCall` part arrives
//! whole: a synthesized `ToolUse` block then a SINGLE `JsonDelta`, left open to close
//! at the terminal drain — and an `inlineData` part (a model-RETURNED image, bl-0987)
//! follows the same whole-part discipline as an `Image` block. `super::decode` dispatches into these; the leaf JSON helpers
//! live in `protocol::json`, the synth mechanics (`next_index`/`open_text`/
//! `open_thinking`) in `protocol::synth`.

use serde_json::Value;

use crate::canonical::{ContentKind, Delta, Event};
use crate::protocol::json::{nonempty, text_of, to_json_string};
use crate::protocol::synth::{next_index, open_text, open_thinking};
use crate::protocol::{DecodeState, OpenBlock};

/// One `parts[]` element (§4.4): `text` opens/extends the text block — or, when
/// `thought: true`, the THINKING block (private chain-of-thought, surfaced via
/// `thinkingConfig.includeThoughts`) as `ThinkingDelta`, never `TextDelta`;
/// `functionCall` arrives whole — `ContentStart{ToolUse}` (synth id) then a SINGLE
/// `JsonDelta`, left open to close at the drain; `inlineData` (a returned image)
/// likewise — `ContentStart{Image{media_type: mimeType}}` then ONE `ImageDelta(data)`,
/// open until the drain (§4.4). Before that arm the part fell through and the image
/// was silently dropped.
pub(super) fn part_events(part: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    if let Some(t) = nonempty(&part["text"]) {
        let (index, delta) = if part["thought"].as_bool() == Some(true) {
            (
                open_thinking(state, out),
                Delta::ThinkingDelta(t.to_owned()),
            )
        } else {
            (open_text(state, out), Delta::TextDelta(t.to_owned()))
        };
        out.push(Event::ContentDelta { index, delta });
    }
    if let Some(call) = part.get("functionCall").filter(|c| c.is_object()) {
        let index = next_index(state);
        let kind = ContentKind::ToolUse {
            id: format!("call_{index}"), // deterministic synth id (§4.5)
            name: text_of(call, "name"),
        };
        state.open.insert(index, OpenBlock { kind: kind.clone() });
        out.push(Event::ContentStart { index, kind });
        out.push(Event::ContentDelta {
            index,
            delta: Delta::JsonDelta(to_json_string(&call["args"])),
        });
        // `thoughtSignature` is a sibling of `functionCall` on the part — LOAD-BEARING
        // for Gemini 2.5 multi-turn function calling (§4.4, bl-61a9). Surface it as a
        // SignatureDelta on the tool block; a sink folds it onto ToolUse.signature.
        if let Some(sig) = nonempty(&part["thoughtSignature"]) {
            out.push(Event::ContentDelta {
                index,
                delta: Delta::SignatureDelta(sig.to_owned()),
            });
        }
    }
    // A missing `mimeType`/`data` reads as the empty string (`text_of`, the lenient
    // path every other block takes) — the block still opens, so identity precedes
    // whatever content the wire did carry; a sink decodes an empty payload to no bytes.
    if let Some(img) = part.get("inlineData").filter(|c| c.is_object()) {
        let index = next_index(state);
        let kind = ContentKind::Image {
            media_type: text_of(img, "mimeType"),
        };
        state.open.insert(index, OpenBlock { kind: kind.clone() });
        out.push(Event::ContentStart { index, kind });
        out.push(Event::ContentDelta {
            index,
            delta: Delta::ImageDelta(text_of(img, "data")),
        });
    }
}
