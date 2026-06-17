//! RESPONSE projection (openai-chat-mapping §3/§4): one parsed SSE frame → ≥0
//! canonical `Event`s. Positional `choices[0].delta`; `MessageStart`/`ContentStart`
//! are synthesized (OpenAI gives no block-start), `arguments` stream as `JsonDelta`
//! fragments (never parsed mid-stream), `[DONE]` flips `terminated`, and a non-2xx
//! whole-body frame surfaces the raw error body (the shared `json::http_error`).
//! The content-block + finish events live in [`blocks`]; this module owns the
//! dispatch, usage, and helpers. Pure over `(frame, &mut state)`; `decode` never
//! emits `End` (run owns it, §3.6).

use serde_json::Value;

use crate::canonical::{CanonicalError, Event, Role, Usage};
use crate::protocol::json::{http_error, nonempty, parse};
use crate::protocol::{DecodeState, Frame};

mod blocks;

/// Decode one frame (§3.3): a non-2xx whole-body frame is the error envelope (§4),
/// `[DONE]` is the terminal marker, anything else is a `chat.completion.chunk`.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if let Some(status) = frame.status {
        // The status is authoritative (§4); the raw body rides provider_detail
        // verbatim (shared `http_error`), so a provider error is never empty.
        return Ok(vec![Event::Error(http_error(&frame.data, status))]); // §4
    }
    if frame.data == b"[DONE]" {
        state.terminated = true; // provider terminal marker; run appends the one End (§3.6)
        return Ok(vec![]);
    }
    chunk(&parse(&frame.data)?, state)
}

/// One `chat.completion.chunk` → events (§3.3). MessageStart fires once on the
/// first chunk; then `choices[0].delta` drives text / tool / refusal / finish, and
/// a populated top-level `usage` (the separate later chunk) yields `Usage`.
fn chunk(v: &Value, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    let mut out = Vec::new();
    if !state.started {
        state.started = true;
        out.push(Event::message_start(
            v["id"].as_str().map(str::to_owned),
            v["model"].as_str().map(str::to_owned),
            Role::Assistant,
        ));
    }
    let choice = &v["choices"][0];
    let delta = &choice["delta"];
    blocks::text(delta, state, &mut out);
    if let Some(r) = nonempty(&delta["refusal"]) {
        state.refusal.push_str(r); // not a content block; surfaces at finish (§3.5)
    }
    if let Some(calls) = delta["tool_calls"].as_array() {
        for call in calls {
            blocks::tool_call(call, state, &mut out);
        }
    }
    if let Some(reason) = choice["finish_reason"].as_str() {
        blocks::finish(reason, state, &mut out); // close every open block, then Finish (§3.3)
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(usage(u))); // emitted after Finish (separate frame, §3.4)
    }
    Ok(out)
}

/// OpenAI `usage` → canonical `Usage` (§3.4): every field `Option`, never a
/// fabricated `0`. `cached_tokens` is `Some` iff `prompt_tokens_details` carried
/// it (absent → `None`, present `0` → `Some(0)`); `cache_write` has no equivalent.
fn usage(u: &Value) -> Usage {
    Usage {
        input: u["prompt_tokens"].as_u64().map(|x| x as u32),
        output: u["completion_tokens"].as_u64().map(|x| x as u32),
        cache_read: u["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .map(|x| x as u32),
        cache_write: None,
    }
}
