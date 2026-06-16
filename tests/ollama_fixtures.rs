//! Golden fixture decode for `ollama_chat` (providers §5.5): each recorded NDJSON
//! stream decodes to the exact canonical `Vec<Event>`, identically under whole-
//! fixture vs one-byte rechunking (arch §9.3). The basic fixture's half of the
//! cross-provider cross-check is asserted in `cross_check_basic.rs`. No network.

use brazen::protocol::ollama_chat::OllamaChat;
use brazen::{
    ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage,
};

const BASIC: &[u8] = include_bytes!("fixtures/ollama_chat_basic.ndjson");
const TOOLS: &[u8] = include_bytes!("fixtures/ollama_chat_tools.ndjson");

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
                input: Some(12),
                output: Some(2),
                cache_read: None, // Ollama reports no cache counters (§5.7)
                cache_write: None,
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
                input: Some(20),
                output: Some(8),
                cache_read: None,
                cache_write: None,
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
