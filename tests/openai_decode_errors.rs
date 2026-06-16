//! Decode coverage for the non-streaming arms (openai-chat-mapping §3.5, §4): the
//! `finish_reason` variants not reached by a full fixture, the non-2xx whole-body
//! error envelope → status-family `ErrorKind`/exit, the `type`/`code` mapping
//! table, malformed-frame → `Transport`, and the `[DONE]` terminal marker. No network.

use brazen::protocol::openai::OpenAiChat;
use brazen::{CanonicalError, DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};

const ERR_401: &[u8] = include_bytes!("fixtures/openai_error_401.json");
const ERR_4XX: &[u8] = include_bytes!("fixtures/openai_error_4xx.json");
const ERR_5XX: &[u8] = include_bytes!("fixtures/openai_error_5xx.json");

/// Decode a single chunk through a fresh state; returns the events. Drives the
/// `finish_reason` arms not exercised by a full fixture (length / function_call).
fn one_chunk(json: &str) -> Vec<Event> {
    let frame = Frame {
        event: None,
        data: json.as_bytes().to_vec(),
        status: None,
    };
    OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap()
}

#[test]
fn finish_reason_length_and_function_call_map() {
    let finish = |reason: &str| {
        let json = format!(
            "{{\"id\":\"c\",\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{reason}\"}}]}}"
        );
        match &one_chunk(&json)[..] {
            [Event::MessageStart { .. }, Event::Finish { reason }] => reason.clone(),
            other => panic!("unexpected: {other:?}"),
        }
    };
    assert_eq!(finish("length"), FinishReason::Length);
    assert_eq!(finish("function_call"), FinishReason::ToolUse); // deprecated alias
}

#[test]
fn error_envelope_maps_the_status_family() {
    // kind derives from the frame's HTTP status (§4.2); the body supplies only
    // message + provider_detail. The status passed here is the one `run` peeks.
    let err = |status: u16, bytes: &[u8]| -> CanonicalError {
        let frame = Frame {
            event: None,
            data: bytes.to_vec(),
            status: Some(status),
        };
        match OpenAiChat
            .decode(frame, &mut DecodeState::default())
            .unwrap()
            .pop()
        {
            Some(Event::Error(e)) => e,
            other => panic!("expected Error, got {other:?}"),
        }
    };

    let e401 = err(401, ERR_401);
    assert_eq!(e401.kind, ErrorKind::Auth);
    assert_eq!(e401.exit_code(), 77);
    assert_eq!(e401.message, "Incorrect API key provided.");
    assert!(e401.provider_detail.is_some());

    let e429 = err(429, ERR_4XX);
    assert_eq!(e429.kind, ErrorKind::Provider { status: 429 });
    assert_eq!(e429.exit_code(), 69);
    assert!(e429.retryable());

    let e500 = err(500, ERR_5XX);
    assert_eq!(e500.kind, ErrorKind::Provider { status: 500 });
    assert_eq!(e500.exit_code(), 70);
    assert!(e500.retryable());
}

#[test]
fn kind_comes_from_status_not_the_body_strings() {
    // The body claims type:"server_error"/code:"invalid_api_key" (which the old
    // string table would have read as 500/Auth), but the authoritative status is
    // 400 — so the kind is Provider{400}. Proves the body strings never drive kind.
    let frame = Frame {
        event: None,
        data: br#"{"error":{"message":"m","type":"server_error","code":"invalid_api_key"}}"#
            .to_vec(),
        status: Some(400),
    };
    match OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap()
        .pop()
    {
        Some(Event::Error(e)) => assert_eq!(e.kind, ErrorKind::Provider { status: 400 }),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn malformed_frame_surfaces_a_transport_error() {
    // No governing status (a mid-stream body): a malformed body is Transport (§4.2).
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        status: None,
    };
    let err = OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}

#[test]
fn non_json_error_body_keeps_the_authoritative_status() {
    // A 5xx whose body is proxy HTML (not JSON): the status is authoritative, so the
    // kind is Provider{502} (exit 70, retryable) — NOT Transport (exit 69). The body
    // is best-effort and degrades to an empty message + no provider_detail (§4.3).
    let frame = Frame {
        event: None,
        data: b"<html>502 Bad Gateway</html>".to_vec(),
        status: Some(502),
    };
    match OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap()
        .pop()
    {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Provider { status: 502 });
            assert_eq!(e.exit_code(), 70);
            assert!(e.retryable());
            assert_eq!(e.message, "");
            assert!(e.provider_detail.is_none());
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn done_marker_sets_terminated_without_an_end_event() {
    let frame = Frame {
        event: None,
        data: b"[DONE]".to_vec(),
        status: None,
    };
    let mut state = DecodeState::default();
    let ev = OpenAiChat.decode(frame, &mut state).unwrap();
    assert!(ev.is_empty()); // decode emits no End
    assert!(state.terminated);
}
