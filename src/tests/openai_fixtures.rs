//! Golden fixture decode (openai-chat-mapping §5): each recorded Chat Completions
//! stream decodes to the exact canonical `Vec<Event>`, identically under
//! whole-fixture vs one-byte rechunking (arch §9.3), and the error envelopes map
//! to the status-family `ErrorKind`/exit. The basic fixture's half of the
//! cross-check is asserted in `cross_check_basic.rs`. No network.

use crate::protocol::openai::OpenAiChat;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_basic.sse");
const USAGE: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_usage.sse");
const TOOLS: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_tools.sse");
const REFUSAL_FILTER: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_refusal_filter.sse");
const REFUSAL_FIELD: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_refusal_field.sse");
const OTHER_FINISH: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_other_finish.sse");

/// Frame the SSE bytes (one chunk, or one byte at a time) then decode the whole
/// stream, appending the single run-owned `End`. Returns events + `terminated`.
fn decode_all(bytes: &[u8], one_byte: bool) -> (Vec<Event>, bool) {
    let mut dec = Framing::Sse.decoder();
    let mut frames = Vec::new();
    if one_byte {
        for b in bytes {
            frames.extend(dec.push(vec![*b]).unwrap());
        }
    } else {
        frames.extend(dec.push(bytes.to_vec()).unwrap());
    }
    frames.extend(dec.finish().unwrap());

    let mut state = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(OpenAiChat.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§3.6); decode emits none
    (events, state.terminated)
}

/// Decode + assert determinism under adversarial one-byte rechunking, then assert
/// the universal invariants (§5): exactly one End (decode emits zero), every
/// `ContentDelta.index` bracketed by a start and a stop.
fn golden(bytes: &[u8]) -> (Vec<Event>, bool) {
    let whole = decode_all(bytes, false);
    assert_eq!(
        decode_all(bytes, true),
        whole,
        "diverged under one-byte rechunk"
    );
    let (events, _) = &whole;
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::End)).count(),
        1,
        "not exactly one End"
    );
    assert!(matches!(events.last(), Some(Event::End)));
    let mut open = std::collections::HashSet::new();
    for e in events {
        match e {
            Event::ContentStart { index, .. } => assert!(open.insert(*index)),
            Event::ContentDelta { index, .. } => {
                assert!(open.contains(index), "delta outside block")
            }
            Event::ContentStop { index } => assert!(open.remove(index)),
            _ => {}
        }
    }
    assert!(open.is_empty(), "a content block never closed");
    whole
}

fn start(id: &str) -> Event {
    Event::message_start(
        Some(id.into()),
        Some("gpt-4o-2024-08-06".into()),
        Role::Assistant,
    )
}
fn text_start(i: u32) -> Event {
    Event::ContentStart {
        index: i,
        kind: ContentKind::Text {},
    }
}
fn text(i: u32, t: &str) -> Event {
    Event::ContentDelta {
        index: i,
        delta: Delta::TextDelta(t.into()),
    }
}

#[test]
fn framing_is_sse() {
    assert_eq!(OpenAiChat.framing(), Framing::Sse);
}

#[test]
fn basic_text_decodes_without_usage() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert!(!ev.iter().any(|e| matches!(e, Event::Usage(_)))); // no include_usage
    assert_eq!(
        ev,
        vec![
            start("chatcmpl-9"),
            text_start(0),
            text(0, "Hel"),
            text(0, "lo"),
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn usage_chunk_decodes_after_finish_with_cached_zero() {
    let (ev, term) = golden(USAGE);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("chatcmpl-9"),
            text_start(0),
            text(0, "Hel"),
            text(0, "lo"),
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(2),
                cache_read_tokens: Some(0), // cached_tokens:0 → Some(0), never None (§3.4)
                cache_write_tokens: None,
            }),
            Event::End,
        ]
    );
}

#[test]
fn tool_call_streams_fragments_identity_first() {
    let (ev, term) = golden(TOOLS);
    assert!(term);
    let jdelta = |t: &str| Event::ContentDelta {
        index: 0,
        delta: Delta::JsonDelta(t.into()),
    };
    assert_eq!(
        ev,
        vec![
            start("chatcmpl-t"),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ToolUse {
                    id: "call_x".into(),
                    name: "get_weather".into()
                },
            },
            // first chunk's empty "" arguments emit no delta (determinism, §3.3)
            jdelta("{\""),
            jdelta("location"),
            jdelta("\":\"Paris\"}"),
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
    // The concatenated fragments parse to the expected input only at fold time.
    let joined = "{\"".to_owned() + "location" + "\":\"Paris\"}";
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&joined).unwrap(),
        serde_json::json!({"location": "Paris"})
    );
}

#[test]
fn content_filter_is_a_refusal_finish_not_an_error() {
    let (ev, term) = golden(REFUSAL_FILTER);
    assert!(term);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error(_)))); // HTTP 200, exit 0
    assert_eq!(
        ev,
        vec![
            start("chatcmpl-cf"),
            text_start(0),
            text(0, "I"),
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::Refusal {
                    category: "content_filter".into(),
                    explanation: None,
                }
            },
            Event::End,
        ]
    );
}

#[test]
fn streamed_refusal_field_accumulates_and_wins_over_finish() {
    let (ev, term) = golden(REFUSAL_FIELD);
    assert!(term);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error(_))));
    assert_eq!(
        ev,
        vec![
            start("chatcmpl-rf"),
            // no content block; refusal is not a ContentDelta
            Event::Finish {
                reason: FinishReason::Refusal {
                    category: "refusal".into(),
                    explanation: Some("I'm sorry, I can't help with that.".into()),
                }
            },
            Event::End,
        ]
    );
}

#[test]
fn unknown_finish_reason_preserved_verbatim() {
    let (ev, _) = golden(OTHER_FINISH);
    assert_eq!(
        ev.iter()
            .find(|e| matches!(e, Event::Finish { .. }))
            .unwrap(),
        &Event::Finish {
            reason: FinishReason::Other("banana".into())
        }
    );
}
