//! Golden fixture decode for `google_generative_ai` (providers §4.4): each recorded
//! `streamGenerateContent` SSE stream decodes to the exact canonical `Vec<Event>`,
//! identically under whole-fixture vs one-byte rechunking (arch §9.3). The basic
//! fixture's half of the cross-provider cross-check is in `cross_check_basic.rs`.
//! No network.

use brazen::protocol::google_genai::GoogleGenAi;
use brazen::{
    ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage,
};

const BASIC: &[u8] = include_bytes!("fixtures/google_genai_basic.sse");
const TOOLS: &[u8] = include_bytes!("fixtures/google_genai_tools.sse");

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
        events.extend(GoogleGenAi.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§4.4)
    (events, state.terminated)
}

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
    Event::message_start(None, Some("gemini-1.5-flash".into()), Role::Assistant)
}

#[test]
fn framing_is_sse() {
    assert_eq!(GoogleGenAi.framing(), Framing::Sse);
}

#[test]
fn basic_text_synthesizes_block_and_finishes_on_the_last_chunk() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(), // Google streams no id → MessageStart id is None (§4.4)
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
                input: Some(5),
                output: Some(2),
                cache_read: None,
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
fn whole_function_call_synthesizes_id_and_promotes_to_tool_use() {
    let (ev, term) = golden(TOOLS);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ToolUse {
                    id: "call_0".into(), // synthesized — Google sends none (§4.5)
                    name: "get_weather".into(),
                },
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::JsonDelta("{\"location\":\"Paris\"}".into()),
            },
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input: Some(10),
                output: Some(5),
                cache_read: Some(3), // cachedContentTokenCount → cache_read (§4.6)
                cache_write: None,
            }),
            // Google reports STOP even on a tool call; the adapter promotes (§4.7)
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}
