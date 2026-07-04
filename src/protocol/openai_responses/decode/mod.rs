//! RESPONSE projection (providers §3.4): one parsed SSE frame → ≥0 canonical
//! `Event`s, dispatched on `data.type`. The wire carries explicit block structure
//! (`output_item`/`content_part` add/done), so the canonical index keys off the
//! `(output_index, content_index)` pair (`state.part_index`, assigned on first sight,
//! only grows) — a `message` item's several content parts each get their own block
//! where the bare `output_index` would collide them. `response.completed` is the
//! native terminator. Pure over `(frame, &mut state)`; `decode` never emits `End`
//! (run owns it, §3.4).

use serde_json::Value;

use crate::canonical::{CanonicalError, ContentKind, Delta, Event, Role};
use crate::protocol::json::{http_error, parse, text_of, u32_at};
use crate::protocol::{DecodeState, Frame, OpenBlock};

mod full;
mod terminal;

pub(super) use full::decode_full;

/// Decode one frame (§3.4): a non-2xx whole-body frame surfaces the raw error body
/// (the shared `http_error`, status-authoritative) — checked BEFORE parsing, so a
/// non-JSON error body keeps its status instead of collapsing to a parse Transport.
/// Anything else is a typed `response.*` event dispatched on `data.type`.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if let Some(status) = frame.status {
        return Ok(vec![Event::Error(http_error(&frame.data, status))]); // §3.7
    }
    Ok(event(&parse(&frame.data)?, state))
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
        "response.reasoning_text.delta" => delta(v, state, Delta::ThinkingDelta), // raw CoT channel (§3.4, CR-R4)
        "response.refusal.delta" => {
            state.refusal.push_str(&text_of(v, "delta")); // surfaces at completion (§3.4)
            vec![]
        }
        "response.output_item.done" => item_done(v, state),
        "response.completed" => terminal::completed(v, state),
        "response.incomplete" => terminal::incomplete(v, state),
        "response.failed" | "response.error" => vec![Event::Error(terminal::stream_error(v))], // §3.7
        // Inner reasoning `.done`/`.part` events fall here as deliberate no-ops (§3.4,
        // CR-R4), mirroring the content_part.done / output_text.done no-ops: the Thinking
        // block closes on the outermost output_item.done, and reasoning_summary_part.added
        // is the deferred per-part open seam (opens nothing today). Pinned in tests.
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
/// `ContentStart{ToolUse}` and a `reasoning` item `ContentStart{Thinking}` — both
/// **identity before content** (the item IS the block; its deltas carry no
/// `content_index` → pair `(output_index, 0)`). A `message` item opens lazily on its
/// first content part, so it yields nothing here.
fn item_added(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let item = &v["item"];
    let kind = match item["type"].as_str() {
        Some("function_call") => ContentKind::ToolUse {
            id: text_of(item, "call_id"),
            name: text_of(item, "name"),
        },
        Some("reasoning") => ContentKind::Thinking {},
        _ => return vec![],
    };
    let index = canonical(state, part_key(v)); // carries no content_index → 0
    open(state, index, kind.clone());
    vec![Event::ContentStart { index, kind }]
}

/// `response.content_part.added` (§3.4): an `output_text` part synthesizes
/// `ContentStart{Text}` at the canonical index for its `(output_index, content_index)` pair.
fn part_added(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    if v["part"]["type"].as_str() != Some("output_text") {
        return vec![];
    }
    let index = canonical(state, part_key(v));
    open(state, index, ContentKind::Text {});
    vec![Event::ContentStart {
        index,
        kind: ContentKind::Text {},
    }]
}

/// A `*.delta` event → `ContentDelta` at the block for its `(output_index, content_index)`
/// pair (§3.4). Unopened/closed → nothing; an open block emits the fragment DIRECTLY as a
/// `ContentDelta` — never accumulated on the block, never parsed mid-stream.
fn delta(v: &Value, state: &mut DecodeState, wrap: fn(String) -> Delta) -> Vec<Event> {
    let Some(&index) = state.part_index.get(&part_key(v)) else {
        return vec![]; // a delta before its block opened routes nowhere
    };
    if !state.open.contains_key(&index) {
        return vec![]; // the block opened then closed: route nowhere
    }
    let frag = text_of(v, "delta");
    vec![Event::ContentDelta {
        index,
        delta: wrap(frag),
    }]
}

/// `response.output_item.done` (the item-level close) → `ContentStop` for EVERY
/// still-open block of that item, ascending (§3.4): a multi-part `message` maps to
/// several canonical blocks, all closed here; an untracked item yields nothing.
fn item_done(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let oi = u32_at(v, "output_index");
    let mut indices: Vec<u32> = state
        .part_index
        .iter()
        .filter(|((o, _), c)| *o == oi && state.open.contains_key(c))
        .map(|(_, &c)| c)
        .collect();
    indices.sort_unstable();
    indices
        .into_iter()
        .map(|index| {
            state.open.remove(&index);
            Event::ContentStop { index }
        })
        .collect()
}

/// Open a block at the canonical `index` with `kind`.
fn open(state: &mut DecodeState, index: u32, kind: ContentKind) {
    state.open.insert(index, OpenBlock { kind });
}

/// The canonical index for a `(output_index, content_index)` pair — looked up, or
/// assigned on first sight (the pair map only grows, so its `len` is the next index).
fn canonical(state: &mut DecodeState, key: (u32, u32)) -> u32 {
    let next = state.part_index.len() as u32;
    *state.part_index.entry(key).or_insert(next)
}

/// An event's `(output_index, content_index)` — the block key. `content_index` is
/// absent on function_call items (the item IS the block) → `0`, never colliding a
/// message item's parts since the two never share an `output_index`.
fn part_key(v: &Value) -> (u32, u32) {
    (u32_at(v, "output_index"), u32_at(v, "content_index"))
}
