//! `Protocol::pinned_window()` — the first rung of the window ladder (model-discovery
//! §5.5): the context size THIS request's own body states, for the one dialect that has
//! a wire field for it. The sibling of `protocol_tuning.rs`/`protocol_shapes.rs`: a
//! declaration about a dialect's own body, proved against every shipped dialect rather
//! than asserted. Pure — no transport, no cache, no clock.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::claude_code::ClaudeCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{CanonicalRequest, Protocol};
use serde_json::json;

/// Every shipped dialect paired with the window it may read out of a body pinning
/// `options.num_ctx` — the whole table in one place, so a new protocol variant is one
/// line here and is judged by the assertion below.
fn expectations() -> Vec<(&'static dyn Protocol, Option<u32>)> {
    vec![
        (&OpenAiChat, None),
        (&OpenAiResponses, None),
        (&AnthropicMessages, None),
        (&GoogleGenAi, None),
        (&OllamaChat, Some(32_768)),
        (&ClaudeCode, None),
    ]
}

/// A request whose `extra` valve carries whatever `options` value is handed in — the
/// exact place a row's `body_defaults = { options = { … } }` lands after the fold.
fn with_options(options: serde_json::Value) -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "options": options,
    }))
    .unwrap()
}

#[test]
fn ollama_reads_the_window_its_body_pins_and_no_other_dialect_does() {
    // The claim in one assertion: `options.num_ctx` is Ollama's context size and means
    // nothing on any other wire, so a request carrying it states a window for exactly
    // one dialect. The default `None` is not a stand-in for "unknown" — the other five
    // genuinely have no body field naming the window.
    let req = with_options(json!({"num_ctx": 32768}));
    for (i, (d, want)) in expectations().into_iter().enumerate() {
        assert_eq!(d.pinned_window(&req), want, "dialect {i}");
    }
}

#[test]
fn a_request_that_pins_nothing_states_nothing() {
    // Carried, never fabricated, at the rung nearest the request: no `options` at all,
    // an `options` object without the key, and an `options` that is not an object
    // (a row may put anything in the valve) each state nothing, so the ladder falls
    // through to what the model states rather than to a repaired number.
    let bare: CanonicalRequest = serde_json::from_value(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
    }))
    .unwrap();
    assert_eq!(OllamaChat.pinned_window(&bare), None);
    assert_eq!(OllamaChat.pinned_window(&with_options(json!({}))), None);
    assert_eq!(OllamaChat.pinned_window(&with_options(json!(7))), None);
}

#[test]
fn a_window_outside_the_representable_range_states_nothing() {
    // A window is a `u32` because the counters it divides are; a zero and a value past
    // the range are both unmeanable, and an unmeanable number is refused rather than
    // clamped — a clamp would put a figure nobody wrote on the wire.
    assert_eq!(
        OllamaChat.pinned_window(&with_options(json!({"num_ctx": 0}))),
        None
    );
    assert_eq!(
        OllamaChat.pinned_window(&with_options(json!({"num_ctx": 4_294_967_296u64}))),
        None
    );
}
