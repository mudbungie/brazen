//! Golden fixture decode for `ollama_chat` (providers §5.5): each recorded NDJSON
//! stream decodes to the exact canonical `Vec<Event>`, identically under whole-
//! fixture vs one-byte rechunking (arch §9.3). The basic fixture's half of the
//! cross-provider cross-check is asserted in `cross_check_basic.rs`. No network.

use crate::protocol::ollama_chat::OllamaChat;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/ollama_chat_basic.ndjson");
const TOOLS: &[u8] = include_bytes!("../../tests/fixtures/ollama_chat_tools.ndjson");
const THINKING: &[u8] = include_bytes!("../../tests/fixtures/ollama_chat_thinking.ndjson");

/// Frame the NDJSON bytes (one chunk, or one byte at a time) then decode the whole
/// stream, appending the single run-owned `End`. Returns events + `terminated`.
fn decode_all(bytes: &[u8], one_byte: bool) -> (Vec<Event>, bool) {
    let mut dec = Framing::Ndjson.decoder();
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
        events.extend(OllamaChat.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§5.5); decode emits none
    (events, state.terminated)
}

/// Decode + assert determinism under one-byte rechunking, then the universal
/// invariants: exactly one End, every `ContentDelta.index` bracketed start..stop.
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

fn start() -> Event {
    Event::message_start(None, Some("llama3.2".into()), Role::Assistant)
}

#[test]
fn framing_is_ndjson() {
    assert_eq!(OllamaChat.framing(), Framing::Ndjson);
}

#[test]
fn basic_text_decodes_with_usage_and_synthesized_start() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hel".into())
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("lo".into())
            },
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(2),
                cache_read_tokens: None, // Ollama reports no cache counters (§5.7)
                cache_write_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn whole_tool_call_synthesizes_id_and_promotes_finish_to_tool_use() {
    let (ev, term) = golden(TOOLS);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ToolUse {
                    id: "call_0".into(), // synthesized — Ollama sends none (§5.6)
                    name: "get_weather".into(),
                },
            },
            // the WHOLE arguments object arrives as a SINGLE JsonDelta (§5.6)
            Event::ContentDelta {
                index: 0,
                delta: Delta::JsonDelta("{\"location\":\"Paris\"}".into()),
            },
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input_tokens: Some(20),
                output_tokens: Some(8),
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            // done_reason is "stop", but an open tool block promotes to ToolUse (§5.8)
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
    let joined = "{\"location\":\"Paris\"}";
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(joined).unwrap(),
        serde_json::json!({ "location": "Paris" })
    );
}

#[test]
fn thinking_surfaces_as_a_thinking_block_before_text() {
    // `message.thinking` (§5.5, when `think` enabled) opens a lazy THINKING block
    // (index 0, before the answer's text block) and streams `ThinkingDelta`s; both
    // blocks drain ascending at `done:true`. A content-only line opens no thinking.
    let (ev, term) = golden(THINKING);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            Event::message_start(None, Some("llama3.2".into()), Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking { id: None }
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta("Let me ".into())
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta("think.".into())
            },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::Text {}
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::TextDelta("Hi".into())
            },
            Event::ContentStop { index: 0 }, // drain is ascending (§5.5)
            Event::ContentStop { index: 1 },
            Event::Usage(Usage {
                input_tokens: Some(5),
                output_tokens: Some(3),
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}
