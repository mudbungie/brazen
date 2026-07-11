//! `openai_chat` ingress response ENCODE (ingress.md §2, §9, §10): the canonical
//! event stream → the client's `chat.completion(.chunk)` wire — the egress
//! `protocol::openai` decode read right-to-left (openai-chat-mapping §3). ONE
//! fold serves both client shapes (§10): every event mutates the shared
//! accumulators (the aggregate IS the stream accumulated); the SSE shape
//! additionally renders each event's frame, and `End` renders `[DONE]` or the
//! folded body. Blocks this dialect cannot carry (reasoning, server tools) emit
//! no client bytes; their opaque replay payloads accumulate toward the stash
//! join point instead (§5). This module decides WHAT each event means; the wire
//! renderings (frames, bodies, the §9 error masquerade) live in [`chunks`].

use serde_json::json;

use super::chunks;
use crate::canonical::{Content, ContentKind, Delta, Event, FinishReason};
use crate::ingress::state::{IngressState, Slot, ThinkAcc, ToolAcc};

/// One canonical event → zero or more client bytes (ingress.md §2). Total —
/// consumes every event including `Error` (§9) — and pure over `(event, state)`.
pub(crate) fn encode_response(event: &Event, state: &mut IngressState) -> Vec<u8> {
    match event {
        Event::MessageStart { id, model, .. } => {
            state.id.clone_from(id);
            state.model.clone_from(model);
            // the role-only first delta every OpenAI stream opens with (§3.3)
            chunks::emit(state, json!({"content": "", "role": "assistant"}), None)
        }
        Event::ContentStart { index, kind } => start(*index, kind, state),
        Event::ContentDelta { index, delta } => fragment(*index, delta, state),
        Event::ContentStop { index } => {
            state.close(*index); // no per-block stop on this wire (§3.3 inverted)
            Vec::new()
        }
        Event::Usage(u) => {
            state.usage = Some(chunks::usage_json(u));
            if state.include_usage {
                chunks::emit_usage(state) // the post-finish usage chunk (§3.4)
            } else {
                Vec::new()
            }
        }
        Event::Finish { reason } => finish(reason, state),
        Event::Error(e) => chunks::error(e, state),
        Event::End => {
            state.finish_stash(); // the turn is whole: emit the §5 (key, payload) pairs
            if state.stream {
                chunks::sentinel(state)
            } else {
                chunks::body(state)
            }
        }
        // --raw-only / forward-compat events: no client slot and no client fact.
        Event::Raw(_) | Event::Other => Vec::new(),
    }
}

/// ContentStart: text and tool blocks open client-side (a tool call's first
/// chunk carries `index` + `id` + `function.name`, §3.3 inverted); reasoning
/// blocks open stash-side (§5); anything else has no slot and is dropped.
fn start(index: u32, kind: &ContentKind, state: &mut IngressState) -> Vec<u8> {
    match kind {
        ContentKind::Text {} => {
            state.slots.insert(index, Slot::Text);
            Vec::new()
        }
        ContentKind::ToolUse { id, name } => {
            let t = state.tools.len(); // insertion order IS the wire tool index
            state.tools.push(ToolAcc {
                id: id.clone(),
                name: name.clone(),
                args: String::new(),
                signature: None,
            });
            state.slots.insert(index, Slot::Tool(t));
            chunks::emit(
                state,
                json!({"tool_calls": [{
                    "function": {"arguments": "", "name": name},
                    "id": id, "index": t, "type": "function"}]}),
                None,
            )
        }
        ContentKind::Thinking { id } => {
            state.slots.insert(
                index,
                Slot::Thinking(ThinkAcc {
                    id: id.clone(),
                    ..ThinkAcc::default()
                }),
            );
            Vec::new()
        }
        ContentKind::RedactedThinking { data } => {
            // opaque, delivered whole at block open — straight to the stash (§5)
            state
                .blocks
                .push(Content::RedactedThinking { data: data.clone() });
            state.slots.insert(index, Slot::Skip);
            Vec::new()
        }
        _ => {
            state.slots.insert(index, Slot::Skip);
            Vec::new()
        }
    }
}

/// ContentDelta, routed by the block's slot: text rides `delta.content`, tool
/// `arguments` ride index-carrying `delta.tool_calls` fragments (concatenated,
/// never parsed mid-stream), reasoning deltas accumulate stash-side and emit
/// nothing, and a delta on an unknown or skipped slot is dropped (the same
/// forward-compat tolerance every decoder in this repo shows unknown wire).
fn fragment(index: u32, delta: &Delta, state: &mut IngressState) -> Vec<u8> {
    let payload = match (state.slots.get_mut(&index), delta) {
        (Some(Slot::Text), Delta::TextDelta(t)) => {
            state.text.push_str(t);
            Some(json!({"content": t}))
        }
        (Some(Slot::Tool(i)), Delta::JsonDelta(a)) => {
            let i = *i;
            state.tools[i].args.push_str(a);
            Some(json!({"tool_calls": [{"function": {"arguments": a}, "index": i}]}))
        }
        (Some(Slot::Tool(i)), Delta::SignatureDelta(s)) => {
            let i = *i; // the Google thoughtSignature on a tool call (§5)
            state.tools[i]
                .signature
                .get_or_insert_with(String::new)
                .push_str(s);
            None
        }
        (Some(Slot::Thinking(t)), Delta::ThinkingDelta(d)) => {
            t.text.push_str(d);
            None
        }
        (Some(Slot::Thinking(t)), Delta::SignatureDelta(s)) => {
            t.signature.get_or_insert_with(String::new).push_str(s);
            None
        }
        (Some(Slot::Thinking(t)), Delta::EncryptedReasoningDelta(e)) => {
            t.encrypted.get_or_insert_with(String::new).push_str(e);
            None
        }
        _ => None,
    };
    payload.map_or_else(Vec::new, |p| chunks::emit(state, p, None))
}

/// Finish (openai-chat-mapping §3.5, inverted): a `Refusal` WITH text re-streams
/// the model's structured `refusal` channel — its own delta chunk, then
/// `finish_reason:"stop"`, exactly the wire the forward table decodes back to
/// `Refusal{category:"refusal"}` — while a textless refusal is the moderation
/// stop, `"content_filter"`. `StopSequence`/`Pause` have no chat vocabulary and
/// take the wire's own spelling of a completed turn (`"stop"` — chat reports a
/// stop-sequence hit as `"stop"` too, §3.5); `Other` passes verbatim.
fn finish(reason: &FinishReason, state: &mut IngressState) -> Vec<u8> {
    let (refusal, fr) = match reason {
        FinishReason::Stop | FinishReason::StopSequence | FinishReason::Pause => (None, "stop"),
        FinishReason::Length => (None, "length"),
        FinishReason::ToolUse => (None, "tool_calls"),
        FinishReason::Refusal {
            explanation: Some(t),
            ..
        } => (Some(t.clone()), "stop"),
        FinishReason::Refusal { .. } => (None, "content_filter"),
        FinishReason::Other(s) => (None, s.as_str()),
    };
    let fr = fr.to_owned();
    let mut out = Vec::new();
    if let Some(t) = refusal {
        state.refusal.push_str(&t);
        out.extend(chunks::emit(state, json!({"refusal": t}), None));
    }
    out.extend(chunks::emit(state, json!({}), Some(&fr)));
    state.finish = Some(fr);
    out
}
