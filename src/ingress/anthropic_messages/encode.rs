//! `anthropic_messages` ingress response ENCODE (ingress.md §2, §9, §10): the canonical
//! event stream → the anthropic-native `POST /v1/messages` wire — the egress
//! `protocol::anthropic` decode read right-to-left (anthropic-messages §3). ONE fold
//! serves both client shapes (§10): every event folds into the [`acc`] accumulator (the
//! non-stream `message` body IS the stream accumulated); the SSE shape additionally
//! renders each event's `event:`/`data:` frame, and `End` renders `message_stop` or the
//! folded body. Because Anthropic natively carries thinking/redacted/server-tool blocks,
//! they are emitted as REAL wire blocks (never the §5 stash — idle for this dialect).
//! This module decides WHAT each event means; the fold lives in [`acc`], the wire
//! renderings in [`frames`].

use serde_json::{json, Value};

use super::acc::{finish_block, merge_usage, open_block, OpenBlock};
use super::frames;
use crate::canonical::{ContentKind, Delta, Event};
use crate::ingress::state::IngressState;

pub(crate) use super::acc::AnthAcc;

/// One canonical event → zero or more client bytes (ingress.md §2). Total — consumes
/// every event including `Error` (§9) — and pure over `(event, state)`.
pub(crate) fn encode_response(event: &Event, state: &mut IngressState) -> Vec<u8> {
    match event {
        Event::MessageStart { id, model, .. } => {
            state.id.clone_from(id);
            state.model.clone_from(model);
            emit(state, "message_start", |st| {
                json!({"type": "message_start",
                    "message": frames::message_object(st, frames::usage_json(&st.anth.usage))})
            })
        }
        Event::ContentStart { index, kind } => start(*index, kind, state),
        Event::ContentDelta { index, delta } => fragment(*index, delta, state),
        Event::ContentStop { index } => stop(*index, state),
        Event::Usage(u) => {
            merge_usage(&mut state.anth.usage, u);
            Vec::new() // usage rides message_delta / the body, never its own event (§3.6)
        }
        Event::Finish { reason } => {
            let (r, details) = frames::stop_reason(reason);
            state.anth.stop_reason = Some(r);
            state.anth.stop_details = details;
            emit(state, "message_delta", message_delta)
        }
        Event::Error(e) => {
            let (status, envelope) = frames::error(e);
            state.status = Some(status);
            let out = if state.stream {
                frames::frame(state, "error", &envelope)
            } else {
                Vec::new()
            };
            state.error = Some(envelope);
            out
        }
        Event::End => {
            if state.stream {
                // A mid-stream error is terminal and NOT followed by message_stop
                // (the wire closes after it, §3.8); a clean stream ends on message_stop.
                if state.error.is_some() {
                    Vec::new()
                } else {
                    emit(state, "message_stop", |_| json!({"type": "message_stop"}))
                }
            } else {
                body(state)
            }
        }
        // --raw-only / forward-compat events: no client slot and no client fact.
        Event::Raw(_) | Event::Other => Vec::new(),
    }
}

/// Render an SSE frame on the stream shape (folding `build`); nothing on the aggregate
/// shape — the same fold already happened, the body renders at `End` (§10).
fn emit(state: &mut IngressState, name: &str, build: impl Fn(&IngressState) -> Value) -> Vec<u8> {
    if !state.stream {
        return Vec::new();
    }
    let data = build(state);
    frames::frame(state, name, &data)
}

/// ContentStart: open the block in the fold and (stream shape) frame its
/// `content_block_start`. An unknown kind has no anthropic slot → tracked as `Skip`.
fn start(index: u32, kind: &ContentKind, state: &mut IngressState) -> Vec<u8> {
    let (block, cb) = open_block(kind);
    state.anth.open.insert(index, block);
    match cb {
        Some(cb) => emit(
            state,
            "content_block_start",
            |_| json!({"type": "content_block_start", "index": index, "content_block": cb}),
        ),
        None => Vec::new(),
    }
}

/// ContentDelta: fold the fragment into its open block and (stream shape) frame the
/// matching `content_block_delta`. A delta with no anthropic slot
/// (`EncryptedReasoningDelta`, `Other`) or on an unknown/closed index emits nothing.
fn fragment(index: u32, delta: &Delta, state: &mut IngressState) -> Vec<u8> {
    let wire = match (state.anth.open.get_mut(&index), delta) {
        (Some(OpenBlock::Text(t)), Delta::TextDelta(d)) => {
            t.push_str(d);
            Some(json!({"type": "text_delta", "text": d}))
        }
        (Some(OpenBlock::Tool { args, .. }), Delta::JsonDelta(d)) => {
            args.push_str(d);
            Some(json!({"type": "input_json_delta", "partial_json": d}))
        }
        (Some(OpenBlock::Thinking { text, .. }), Delta::ThinkingDelta(d)) => {
            text.push_str(d);
            Some(json!({"type": "thinking_delta", "thinking": d}))
        }
        (Some(OpenBlock::Thinking { signature, .. }), Delta::SignatureDelta(d)) => {
            signature.push_str(d);
            Some(json!({"type": "signature_delta", "signature": d}))
        }
        _ => None,
    };
    match wire {
        Some(d) => emit(
            state,
            "content_block_delta",
            |_| json!({"type": "content_block_delta", "index": index, "delta": d}),
        ),
        None => Vec::new(),
    }
}

/// ContentStop: finalize the block into the fold's `content` (aggregate body) and
/// (stream shape) frame its `content_block_stop`. A `Skip` block (unknown kind) opened
/// with no `content_block_start` frame, so it closes with none either — no wire trace.
fn stop(index: u32, state: &mut IngressState) -> Vec<u8> {
    let Some(block) = state.anth.open.remove(&index) else {
        return Vec::new();
    };
    match finish_block(block) {
        Some(v) => {
            state.anth.content.push(v);
            emit(
                state,
                "content_block_stop",
                |_| json!({"type": "content_block_stop", "index": index}),
            )
        }
        None => Vec::new(),
    }
}

/// The terminal `message_delta`: the mapped `stop_reason` (+ refusal `stop_details`)
/// and the cumulative usage (§3.4, inverted).
fn message_delta(state: &IngressState) -> Value {
    let mut delta = json!({"stop_reason": state.anth.stop_reason, "stop_sequence": null});
    if let Some(sd) = &state.anth.stop_details {
        delta["stop_details"] = sd.clone();
    }
    json!({"type": "message_delta", "delta": delta,
        "usage": frames::usage_json(&state.anth.usage)})
}

/// The non-stream aggregate body (§10): the stream, accumulated — no second code path.
/// An encoded `Error` event replaces it with the §9 envelope. Either shape carries the
/// §4 exposure: a top-level `"brazen":{"adaptations":[…]}` field when a lossy
/// adaptation fired (typed SDKs drop the unknown field harmlessly).
fn body(state: &IngressState) -> Vec<u8> {
    let mut v = match &state.error {
        Some(e) => e.clone(),
        None => success_body(state),
    };
    if !state.adaptations.is_empty() {
        v["brazen"] = json!({"adaptations": state.adaptations});
    }
    v.to_string().into_bytes()
}

/// The folded `message` object (§3, inverted from `decode_full`'s explode): the
/// accumulated blocks made whole, the terminal stop reason/details, the final usage.
fn success_body(state: &IngressState) -> Value {
    let mut v = json!({
        "type": "message",
        "id": frames::wire_id(state),
        "role": "assistant",
        "model": state.wire_model(),
        "content": state.anth.content,
        "stop_reason": state.anth.stop_reason,
        "stop_sequence": null,
        "usage": frames::usage_json(&state.anth.usage),
    });
    if let Some(sd) = &state.anth.stop_details {
        v["stop_details"] = sd.clone();
    }
    v
}
