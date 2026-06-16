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
        whole_body: false,
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
    let err = |bytes: &[u8]| -> CanonicalError {
        let frame = Frame {
            event: None,
            data: bytes.to_vec(),
            whole_body: true,
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

    let e401 = err(ERR_401);
    assert_eq!(e401.kind, ErrorKind::Auth);
    assert_eq!(e401.exit_code(), 77);
    assert_eq!(e401.message, "Incorrect API key provided.");
    assert!(e401.provider_detail.is_some());

    let e429 = err(ERR_4XX);
    assert_eq!(e429.kind, ErrorKind::Provider { status: 429 });
    assert_eq!(e429.exit_code(), 69);
    assert!(e429.retryable());

    let e500 = err(ERR_5XX);
    assert_eq!(e500.kind, ErrorKind::Provider { status: 500 });
    assert_eq!(e500.exit_code(), 70);
    assert!(e500.retryable());
}

#[test]
fn error_kind_covers_every_type_and_code_arm() {
    let kind = |body: &str| -> ErrorKind {
        let frame = Frame {
            event: None,
            data: body.as_bytes().to_vec(),
            whole_body: true,
        };
        match OpenAiChat
            .decode(frame, &mut DecodeState::default())
            .unwrap()
            .pop()
        {
            Some(Event::Error(e)) => e.kind,
            other => panic!("expected Error, got {other:?}"),
        }
    };
    let by_code = |code: &str| {
        kind(&format!(
            "{{\"error\":{{\"message\":\"m\",\"code\":\"{code}\"}}}}"
        ))
    };
    let by_type = |ty: &str| {
        kind(&format!(
            "{{\"error\":{{\"message\":\"m\",\"type\":\"{ty}\"}}}}"
        ))
    };
    // code takes precedence (a 401 also reads type:invalid_request_error)
    assert_eq!(by_code("invalid_authentication"), ErrorKind::Auth);
    assert_eq!(
        by_code("insufficient_quota"),
        ErrorKind::Provider { status: 429 }
    );
    // type arms
    assert_eq!(by_type("authentication_error"), ErrorKind::Auth);
    assert_eq!(by_type("permission_error"), ErrorKind::Auth);
    assert_eq!(by_type("permission_denied"), ErrorKind::Auth);
    assert_eq!(
        by_type("not_found_error"),
        ErrorKind::Provider { status: 404 }
    );
    assert_eq!(
        by_type("rate_limit_error"),
        ErrorKind::Provider { status: 429 }
    );
    assert_eq!(
        by_type("service_unavailable"),
        ErrorKind::Provider { status: 503 }
    );
    assert_eq!(
        by_type("invalid_request_error"),
        ErrorKind::Provider { status: 400 }
    );
    assert_eq!(by_type("totally_unknown"), ErrorKind::Transport); // safe default
}

#[test]
fn malformed_frame_surfaces_a_transport_error() {
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        whole_body: false,
    };
    let err = OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
    // a malformed whole-body error frame likewise surfaces as Transport, never a panic
    let frame = Frame {
        event: None,
        data: b"<html>502</html>".to_vec(),
        whole_body: true,
    };
    let err = OpenAiChat
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}

#[test]
fn done_marker_sets_terminated_without_an_end_event() {
    let frame = Frame {
        event: None,
        data: b"[DONE]".to_vec(),
        whole_body: false,
    };
    let mut state = DecodeState::default();
    let ev = OpenAiChat.decode(frame, &mut state).unwrap();
    assert!(ev.is_empty()); // decode emits no End
    assert!(state.terminated);
}
