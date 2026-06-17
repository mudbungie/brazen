//! Golden fixture decode (anthropic-messages §5): each recorded stream decodes to
//! the exact canonical `Vec<Event>`, identically under whole-fixture vs one-byte
//! rechunking (one-byte subsumes MidUtf8/MidJsonNumber, arch §9.3), and the basic
//! fixture reduces to the cross-check skeleton (§5.1). No network.

use brazen::protocol::anthropic::AnthropicMessages;
use brazen::{
    CanonicalError, ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason, Frame,
    Framing, Protocol, Role, Usage,
};
use serde_json::json;

const BASIC: &[u8] = include_bytes!("fixtures/anthropic_messages_basic.sse");
const THINKING_TOOLS: &[u8] = include_bytes!("fixtures/anthropic_messages_thinking_tools.sse");
const REFUSAL: &[u8] = include_bytes!("fixtures/anthropic_messages_refusal.sse");
const PAUSE: &[u8] = include_bytes!("fixtures/anthropic_messages_pause.sse");
const OVERLOADED: &[u8] = include_bytes!("fixtures/anthropic_error_overloaded.json");

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
        input,
        output,
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
fn framing_is_sse() {
    assert_eq!(AnthropicMessages.framing(), Framing::Sse);
}

#[test]
fn basic_text_stream_decodes_to_the_3_9_trace() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("msg_01XYZ"),
            usage(Some(12), Some(1)),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            text(0, "Hel"),
            text(0, "lo"),
            Event::ContentStop { index: 0 },
            usage(None, Some(2)),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn thinking_then_tool_use_decodes_natively_identity_first() {
    let (ev, term) = golden(THINKING_TOOLS);
    assert!(term);
    let think = |t: &str| Event::ContentDelta {
        index: 0,
        delta: Delta::ThinkingDelta(t.into()),
    };
    let jdelta = |t: &str| Event::ContentDelta {
        index: 1,
        delta: Delta::JsonDelta(t.into()),
    };
    assert_eq!(
        ev,
        vec![
            start("msg_think"),
            Event::Usage(Usage {
                input: Some(40),
                output: Some(1),
                cache_read: Some(8),
                cache_write: Some(4),
            }),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking {}
            },
            think("Let me"),
            think(" check."),
            // signature_delta emits no event (CR-5)
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "toolu_01A".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(""),
            jdelta("{\"location\":"),
            jdelta("\"SF\"}"),
            Event::ContentStop { index: 1 },
            usage(None, Some(20)),
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}

#[test]
fn streamed_refusal_is_a_finish_not_an_error() {
    let (ev, term) = golden(REFUSAL);
    assert!(term);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error(_)))); // HTTP 200, exit 0
    assert_eq!(
        ev,
        vec![
            start("msg_ref"),
            usage(Some(100), Some(1)),
            usage(None, Some(5)),
            Event::Finish {
                reason: FinishReason::Refusal {
                    category: "cyber".into(),
                    explanation: Some("Can't help with that.".into()),
                }
            },
            Event::End,
        ]
    );
}

#[test]
fn pause_turn_drops_the_server_tool_use_block() {
    let (ev, term) = golden(PAUSE);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start("msg_pause"),
            usage(Some(50), Some(1)),
            usage(None, Some(10)),
            Event::Finish {
                reason: FinishReason::Pause
            },
            Event::End,
        ]
    );
}

#[test]
fn non_2xx_whole_body_decodes_to_a_provider_error_exit_70() {
    // The whole-body error frame carries the HTTP status; kind derives from it
    // (§4.3, ErrorKind::from_http_status), not from the body's overloaded_error type.
    // provider_detail carries the WHOLE raw body verbatim (bl-5fe6), envelope and all.
    let frame = Frame {
        event: None,
        data: OVERLOADED.to_vec(),
        status: Some(529),
    };
    let mut state = DecodeState::default();
    let ev = AnthropicMessages.decode(frame, &mut state).unwrap();
    let expect = CanonicalError {
        kind: ErrorKind::Provider { status: 529 },
        message: "Overloaded".into(),
        provider_detail: Some(json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"},
            "request_id": "req_011CSHo",
        })),
    };
    assert_eq!(ev, vec![Event::Error(expect.clone())]);
    assert_eq!(expect.exit_code(), 70);
    assert!(!state.terminated); // a decoded Error is terminal; run owns End, not decode
}

/// `normalize` (§5.1): drop `MessageStart` id/model and every `Usage` event — the
/// one reduction pinned identically on both protocol sides for the cross-check.
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
fn basic_reduces_to_the_cross_check_skeleton() {
    let (ev, _) = decode_all(BASIC, false);
    assert_eq!(
        normalize(ev),
        vec![
            Event::message_start(None, None, Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
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
