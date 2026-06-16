//! Golden fixture decode for `openai_responses` (providers §3.4): each recorded
//! Responses SSE stream decodes to the exact canonical `Vec<Event>`, identically
//! under whole-fixture vs one-byte rechunking (arch §9.3). The basic fixture's half
//! of the cross-provider cross-check is in `cross_check_basic.rs`. No network.

use brazen::protocol::openai_responses::OpenAiResponses;
use brazen::{
    ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage,
};

const BASIC: &[u8] = include_bytes!("fixtures/openai_responses_basic.sse");
const TOOLS: &[u8] = include_bytes!("fixtures/openai_responses_tools.sse");

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
        events.extend(OpenAiResponses.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§3.4)
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

fn start(id: &str) -> Event {
    Event::message_start(
        Some(id.into()),
        Some("gpt-4o-2024-08-06".into()),
        Role::Assistant,
    )
}

#[test]
fn framing_is_sse() {
    assert_eq!(OpenAiResponses.framing(), Framing::Sse);
}

#[test]
fn basic_text_opens_lazily_and_finishes_on_completed() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("resp_1"),
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
                cache_read: Some(0), // cached_tokens:0 → Some(0), never None (§3.5)
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
fn tool_call_streams_argument_fragments_identity_first() {
    let (ev, term) = golden(TOOLS);
    assert!(term);
    let jdelta = |t: &str| Event::ContentDelta {
        index: 0,
        delta: Delta::JsonDelta(t.into()),
    };
    assert_eq!(
        ev,
        vec![
            start("resp_t"),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ToolUse {
                    id: "call_x".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta("{\""), // arguments stream as FRAGMENTS (unlike the whole-call dialects)
            jdelta("location"),
            jdelta("\":\"Paris\"}"),
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input: Some(20),
                output: Some(8),
                cache_read: None, // no input_tokens_details → None (§3.5)
                cache_write: None,
            }),
            Event::Finish {
                reason: FinishReason::ToolUse
            }, // function_call in output (§3.6)
            Event::End,
        ]
    );
}
