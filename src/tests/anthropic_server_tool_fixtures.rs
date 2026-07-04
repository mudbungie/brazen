//! Server-tool golden fixtures (anthropic-messages §5, CR-4 resolved): the pause
//! stream now SURFACES its `server_tool_use` block, and the web_search golden pins
//! the full use→inline-result shape — each decoded identically under whole-fixture
//! vs one-byte rechunking (arch §9.3). No network.

use crate::protocol::anthropic::AnthropicMessages;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};
use serde_json::json;

const PAUSE: &[u8] = include_bytes!("../../tests/fixtures/anthropic_messages_pause.sse");
const WEB_SEARCH: &[u8] = include_bytes!("../../tests/fixtures/anthropic_messages_web_search.sse");

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
        events.extend(AnthropicMessages.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§3.8); decode emits none
    (events, state.terminated)
}

/// Decode + assert determinism under adversarial one-byte rechunking.
fn golden(bytes: &[u8]) -> (Vec<Event>, bool) {
    let whole = decode_all(bytes, false);
    assert_eq!(
        decode_all(bytes, true),
        whole,
        "diverged under one-byte rechunk"
    );
    whole
}

fn start(id: &str) -> Event {
    Event::message_start(
        Some(id.into()),
        Some("claude-opus-4-8".into()),
        Role::Assistant,
    )
}
fn usage(input: Option<u32>, output: Option<u32>) -> Event {
    Event::Usage(Usage {
        input_tokens: input,
        output_tokens: output,
        ..Usage::default()
    })
}
fn text(i: u32, t: &str) -> Event {
    Event::ContentDelta {
        index: i,
        delta: Delta::TextDelta(t.into()),
    }
}

#[test]
fn pause_turn_surfaces_the_server_tool_use_block() {
    // CR-4 resolved: the pause fixture's server_tool_use block now SURFACES
    // (start + JsonDelta + stop) before the Finish{Pause}, instead of dropping.
    let (ev, term) = golden(PAUSE);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("msg_pause"),
            usage(Some(50), Some(1)),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ServerToolUse {
                    id: "srvtoolu_1".into(),
                    name: "web_search".into(),
                },
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::JsonDelta("{\"query\":\"x\"}".into()),
            },
            Event::ContentStop { index: 0 },
            usage(None, Some(10)),
            Event::Finish {
                reason: FinishReason::Pause
            },
            Event::End,
        ]
    );
}

#[test]
fn web_search_stream_decodes_use_then_inline_result() {
    // The web-search golden (§5): a server_tool_use block streaming its input as
    // JsonDeltas, then a web_search_tool_result whose FULL content array arrives
    // inline at ContentStart with NO delta before its stop — deterministic under
    // one-byte rechunk like every golden.
    let (ev, term) = golden(WEB_SEARCH);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("msg_websearch"),
            usage(Some(30), Some(1)),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            text(0, "Let me search."),
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ServerToolUse {
                    id: "srvtoolu_1".into(),
                    name: "web_search".into(),
                },
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::JsonDelta("{\"query\":".into()),
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::JsonDelta("\"weather NY\"}".into()),
            },
            Event::ContentStop { index: 1 },
            Event::ContentStart {
                index: 2,
                kind: ContentKind::ServerToolResult {
                    kind: "web_search_tool_result".into(),
                    tool_use_id: "srvtoolu_1".into(),
                    content: json!([{"type": "web_search_result",
                                     "url": "https://example.com/ny-weather",
                                     "title": "NY Weather",
                                     "encrypted_content": "Ev0DCioIAxgC...",
                                     "page_age": null}]),
                },
            },
            Event::ContentStop { index: 2 }, // no delta between start and stop
            Event::ContentStart {
                index: 3,
                kind: ContentKind::Text {}
            },
            text(3, "It is 20C in NY."),
            Event::ContentStop { index: 3 },
            usage(None, Some(25)),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}
