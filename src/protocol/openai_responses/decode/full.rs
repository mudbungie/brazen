//! The non-stream 2xx fold (providers §3.4, config §4.2): a COMPLETE `stream:false`
//! Responses body exploded back into the synthetic `response.*` frame sequence and
//! replayed through the parent's ONE `event` dispatcher — so the typed handlers and
//! `terminal::{completed,usage,…}` run verbatim, no second parser, and the canonical
//! `(output_index, content_index)` block index is identical to the streamed form.

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, Event};
use crate::protocol::json::{parse, text_of};
use crate::protocol::DecodeState;

use super::event;

/// Decode a COMPLETE non-stream 2xx body (config §4.2). A `stream:false` Responses
/// body IS the `response` object — the very one streaming's `response.completed`
/// wraps as `v["response"]`. The explode→replay reconstructs the typed `response.*`
/// events the stream would have sent and drives them through the SAME `event`
/// dispatcher: `response.created` (id/model), then each `output[oi]` item fanned to
/// its `output_item.added`/`content_part.added`/`*.delta`/`output_item.done` frames
/// (a `message`'s parts, a `function_call`'s whole arguments, a `reasoning`'s summary),
/// then `response.completed` — so `terminal::{completed,usage,…}` run verbatim, no
/// second parser. `output_index`/`content_index` are the array positions the wire
/// carried, keeping the canonical `(oi, ci)` index identical to the streamed form.
pub(crate) fn decode_full(
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    let response = parse(body)?;
    let mut out = event(&created(&response), state);
    for (oi, item) in response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        explode_item(oi as u32, item, state, &mut out);
    }
    out.extend(event(&completed(&response), state));
    Ok(out)
}

/// One finished `output[oi]` item → its synthetic stream frames, each driven through
/// the SAME `event` dispatcher (§3.4). A `message` opens lazily per `content[ci]`
/// `output_text` part (one `output_text.delta` of the whole text); a `function_call`
/// is identity-first with one whole-arguments `function_call_arguments.delta`; a
/// `reasoning` is identity-first with one `reasoning_summary_text.delta` per
/// `summary[]` entry — then the item-level `output_item.done` closes every part.
fn explode_item(oi: u32, item: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let added = json!({ "type": "response.output_item.added", "output_index": oi, "item": item });
    out.extend(event(&added, state));
    match item["type"].as_str().unwrap_or_default() {
        "message" => {
            for (ci, part) in item["content"].as_array().into_iter().flatten().enumerate() {
                explode_part(oi, ci as u32, part, state, out);
            }
        }
        "function_call" => out.extend(event(
            &arg_delta(
                oi,
                "response.function_call_arguments.delta",
                &text_of(item, "arguments"),
            ),
            state,
        )),
        "reasoning" => {
            for s in item["summary"].as_array().into_iter().flatten() {
                out.extend(event(
                    &arg_delta(
                        oi,
                        "response.reasoning_summary_text.delta",
                        &text_of(s, "text"),
                    ),
                    state,
                ));
            }
        }
        _ => {}
    }
    let done = json!({ "type": "response.output_item.done", "output_index": oi, "item": item });
    out.extend(event(&done, state));
}

/// One `message` `content[ci]` part → `content_part.added` then one whole-text
/// `output_text.delta` (§3.4) — an `output_text` part opens a Text block; other part
/// kinds open nothing, exactly as the stream's `part_added` no-ops them.
fn explode_part(oi: u32, ci: u32, part: &Value, state: &mut DecodeState, out: &mut Vec<Event>) {
    let added = json!({ "type": "response.content_part.added", "output_index": oi, "content_index": ci, "part": part });
    out.extend(event(&added, state));
    let delta = json!({ "type": "response.output_text.delta", "output_index": oi, "content_index": ci, "delta": part["text"] });
    out.extend(event(&delta, state));
}

/// A whole-content `*.delta` frame at `output_index` (no `content_index`, like the
/// function_call / reasoning item streams) carrying the entire accumulated string.
fn arg_delta(oi: u32, ty: &str, delta: &str) -> Value {
    json!({ "type": ty, "output_index": oi, "delta": delta })
}

/// The synthetic `response.created` frame: the body's `response` object wrapped as
/// the event's `response`, so the existing `message_start` reads its id/model.
fn created(response: &Value) -> Value {
    json!({ "type": "response.created", "response": response })
}

/// The synthetic `response.completed` frame wrapping the body's `response` object —
/// `terminal::completed` reads `v["response"]`, so the drain/usage/finish run verbatim.
fn completed(response: &Value) -> Value {
    json!({ "type": "response.completed", "response": response })
}
