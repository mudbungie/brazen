//! Golden fixture decode (anthropic-messages §5): each recorded stream decodes to
//! the exact canonical `Vec<Event>`, identically under whole-fixture vs one-byte
//! rechunking (one-byte subsumes MidUtf8/MidJsonNumber, arch §9.3), and the basic
//! fixture reduces to the cross-check skeleton (§5.1). The server-tool goldens
//! (pause, web_search) live in `anthropic_server_tool_fixtures`. No network.

use crate::protocol::anthropic::AnthropicMessages;
use crate::{
    CanonicalError, ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason, Frame,
    Framing, Protocol, Role, Usage,
};
use serde_json::json;

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/anthropic_messages_basic.sse");
const THINKING_TOOLS: &[u8] =
    include_bytes!("../../tests/fixtures/anthropic_messages_thinking_tools.sse");
const REFUSAL: &[u8] = include_bytes!("../../tests/fixtures/anthropic_messages_refusal.sse");
const OVERLOADED: &[u8] = include_bytes!("../../tests/fixtures/anthropic_error_overloaded.json");

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
    // Real recorded haiku-4.5 stream (architecture.md §9.2 "recorded from real
    // streams, committed verbatim"): it carries wire details a synthetic golden
    // would elide — `ping` keep-alives (decode yields nothing), a `caller` field
    // on tool_use (ignored), trailing data-line padding, and a real ~700-char
    // signature. The decode is asserted against it byte-for-byte.
    let think = |t: &str| Event::ContentDelta {
        index: 0,
        delta: Delta::ThinkingDelta(t.into()),
    };
    let jdelta = |t: &str| Event::ContentDelta {
        index: 1,
        delta: Delta::JsonDelta(t.into()),
    };
    let sig = "EvEDCpMBCA8YAipA62hxLLCBiRkZIzoHRmYxA1x7T8mZrQCOIppkwY4UhZUXlVhnmPP8UwvfVCx5JxDn2gCWe7k9h+jUR9w2X4tWKDIZY2xhdWRlLWhhaWt1LTQtNS0yMDI1MTAwMTgAQgh0aGlua2luZ1okZGFkYmJiOTQtNDQ2Zi00OWIwLTg3ZGItMWM0ZDdhYjQ4MDU0Egxih9ZfCdG4NUeWVEgaDB0SJBNKY0+gwX1MTyIwk5cwIuMJQe10KlRxBdv+clTsj1t9mCvZfSj6dpG3d1ukb6XpYPcFOGe72Wh3XjYFKooCIYYHgzH+8gSLpSwJ61Jh2F88+hHQptK/dkbsYS9eqkwxDkky9xshnIIq+/EJIwU44Zdw3nmGNVMPNDq6m51nd7mtwBagx9/jJJ6MVAnxfb5HrsLoGtUSly82nNkBj0PsEvJyZbsFUD5avczZugLSH7dfe9VOVWBIDl6t27BMP1xsnj2+Se7d1r82sdOaqy8Lh7PsRgH68sWmVrAM79z6wmmfFN+pN//UDWW7AWwzKf8sTfxHftE4GG++QvLGoabhDvEbekfib5jd+aR/MfZaW9nMZ02mRWzsHR/0TaIU4ZAe82P/Eaqkyr9s+jIf0dI+LSx7I4jrDIP8Yaq+1hR5ZvEfPwymDy8Q9GQYAQ==";
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("msg_011CcuYJJJo24ERaWWjsuhT7".into()),
                Some("claude-haiku-4-5-20251001".into()),
                Role::Assistant,
            ),
            Event::Usage(Usage {
                input_tokens: Some(614),
                output_tokens: Some(8),
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
            }),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking { id: None }
            },
            think("The user is asking me to call get"),
            think("_weather for SF (San Francisco). They want only the tool call, no other response.\n\nI need to call get"),
            think("_weather with location \"SF\" or \"San Francisco\". The user said \"SF\" so I should"),
            think(" use that exactly as they provided it."),
            // signature_delta SURFACES as a SignatureDelta (bl-61a9, CR-5 resolved),
            // in wire order just before the thinking block's stop.
            Event::ContentDelta {
                index: 0,
                delta: Delta::SignatureDelta(sig.into()),
            },
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "toolu_01XXCPWEWpgnJ8BM4hcmSB3e".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(""),
            jdelta("{\"lo"),
            jdelta("cati"),
            jdelta("on\": \""),
            jdelta("SF\"}"),
            Event::ContentStop { index: 1 },
            Event::Usage(Usage {
                input_tokens: Some(614),
                output_tokens: Some(123),
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
            }),
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
        retry_after_seconds: None,
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
