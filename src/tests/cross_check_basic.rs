//! The single-source-of-truth cross-check (architecture.md §3.6, openai §5.1,
//! anthropic §5.1, providers §1): the "basic text" fixture of EVERY shipped protocol
//! represents the SAME logical response (the assistant replying `Hello`, chunked
//! `"Hel"`+`"lo"`, finishing normally). After `normalize` drops provider-inherent
//! identity (`MessageStart` id/model) and every `Usage` event, all reduced
//! `Vec<Event>` must be BYTE-IDENTICAL — the executable proof that the canonical
//! model is one model, not five. No network.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{DecodeState, Event, Framing, Protocol};

/// Frame + decode a whole fixture through `proto` at its own `framing`, appending
/// the run-owned `End`.
fn decode_all(bytes: &[u8], framing: Framing, proto: &dyn Protocol) -> Vec<Event> {
    let mut dec = framing.decoder();
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

/// `normalize` (§5.1): drop `MessageStart` id/model (provider-inherent identity) and
/// every `Usage` event (each provider streams usage on its own schedule) — the one
/// reduction pinned identically across all protocols. It drops nothing else.
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
fn every_protocol_basic_fixture_decodes_to_the_same_events() {
    let reduced =
        |bytes: &[u8], framing, proto: &dyn Protocol| normalize(decode_all(bytes, framing, proto));
    let openai = reduced(
        include_bytes!("../../tests/fixtures/openai_chat_basic.sse"),
        Framing::Sse,
        &OpenAiChat,
    );
    let cases: [(Vec<Event>, &str); 4] = [
        (
            reduced(
                include_bytes!("../../tests/fixtures/anthropic_messages_basic.sse"),
                Framing::Sse,
                &AnthropicMessages,
            ),
            "anthropic",
        ),
        (
            reduced(
                include_bytes!("../../tests/fixtures/openai_responses_basic.sse"),
                Framing::Sse,
                &OpenAiResponses,
            ),
            "openai_responses",
        ),
        (
            reduced(
                include_bytes!("../../tests/fixtures/google_genai_basic.sse"),
                Framing::Sse,
                &GoogleGenAi,
            ),
            "google_generative_ai",
        ),
        (
            reduced(
                include_bytes!("../../tests/fixtures/ollama_chat_basic.ndjson"),
                Framing::Ndjson,
                &OllamaChat,
            ),
            "ollama_chat",
        ),
    ];
    for (events, name) in cases {
        assert_eq!(
            events, openai,
            "{name} basic fixture must reduce to the one canonical Vec<Event>"
        );
    }
}
