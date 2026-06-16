//! The single-source-of-truth cross-check (architecture.md §3.6, openai §5.1,
//! anthropic §5.1): the OpenAI-basic and Anthropic-basic fixtures represent the
//! SAME logical "basic text" response (the assistant replying `Hello`, chunked
//! `"Hel"`+`"lo"`, finishing normally). After `normalize` drops provider-inherent
//! identity (`MessageStart` id/model) and every `Usage` event, the two reduced
//! `Vec<Event>` must be BYTE-IDENTICAL — the executable proof that the canonical
//! model is one model, not two. No network.

use brazen::protocol::anthropic::AnthropicMessages;
use brazen::protocol::openai::OpenAiChat;
use brazen::{DecodeState, Event, Framing, Protocol};

const OPENAI_BASIC: &[u8] = include_bytes!("fixtures/openai_chat_basic.sse");
const ANTHROPIC_BASIC: &[u8] = include_bytes!("fixtures/anthropic_messages_basic.sse");

/// Frame + decode a whole SSE fixture through `proto`, appending the run-owned `End`.
fn decode_all(bytes: &[u8], proto: &dyn Protocol) -> Vec<Event> {
    let mut dec = Framing::Sse.decoder();
    let mut frames = dec.push(bytes.to_vec()).unwrap();
    frames.extend(dec.finish().unwrap());
    let mut state = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(proto.decode(f, &mut state).unwrap());
    }
    events.push(Event::End);
    events
}

/// `normalize` (§5.1): drop `MessageStart` id/model (provider-inherent identity)
/// and every `Usage` event (omitted on OpenAI's basic, dropped from Anthropic's) —
/// the one reduction pinned identically on both protocol sides. It drops nothing else.
fn normalize(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .filter(|e| !matches!(e, Event::Usage(_)))
        .map(|e| match e {
            Event::MessageStart { role, .. } => Event::message_start(None, None, role),
            other => other,
        })
        .collect()
}

#[test]
fn openai_and_anthropic_basic_decode_to_the_same_events() {
    let openai = normalize(decode_all(OPENAI_BASIC, &OpenAiChat));
    let anthropic = normalize(decode_all(ANTHROPIC_BASIC, &AnthropicMessages));
    assert_eq!(
        openai, anthropic,
        "the two basic fixtures must reduce to one canonical Vec<Event>"
    );
}
