//! Decode coverage for `ollama_chat` arms not reached by a full fixture (providers
//! §5.5/§5.8/§5.9): the `done_reason` variants, the non-2xx bare-string error
//! envelope, a mid-stream `{"error":…}` line, and a malformed line → `Transport`.
//! No network.

use brazen::protocol::ollama_chat::OllamaChat;
use brazen::{DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};

const ERR: &[u8] = include_bytes!("fixtures/ollama_error.json");

/// Decode a single NDJSON line through a fresh state; returns the events.
fn one_line(json: &str) -> Vec<Event> {
    let frame = Frame {
        event: None,
        data: json.as_bytes().to_vec(),
        status: None,
    };
    OllamaChat
        .decode(frame, &mut DecodeState::default())
        .unwrap()
}

/// The terminal `done:true` line's `Finish` reason for a text-only turn.
fn finish_of(done_reason: &str) -> FinishReason {
    let line = format!(
        "{{\"model\":\"m\",\"message\":{{\"content\":\"\"}},\"done\":true,\"done_reason\":{done_reason}}}"
    );
    match one_line(&line)
        .into_iter()
        .find(|e| matches!(e, Event::Finish { .. }))
    {
        Some(Event::Finish { reason }) => reason,
        other => panic!("expected Finish, got {other:?}"),
    }
}

#[test]
fn done_reason_maps_length_default_and_unknown() {
    assert_eq!(finish_of("\"length\""), FinishReason::Length);
    assert_eq!(finish_of("null"), FinishReason::Stop); // absent done_reason → Stop
    assert_eq!(finish_of("\"abort\""), FinishReason::Other("abort".into()));
}

#[test]
fn whole_body_error_maps_the_status_family() {
    let err = |status: u16| -> brazen::CanonicalError {
        let frame = Frame {
            event: None,
            data: ERR.to_vec(),
            status: Some(status),
        };
        match OllamaChat
            .decode(frame, &mut DecodeState::default())
            .unwrap()
            .pop()
        {
            Some(Event::Error(e)) => e,
            other => panic!("expected Error, got {other:?}"),
        }
    };
    let e404 = err(404);
    assert_eq!(e404.kind, ErrorKind::Provider { status: 404 });
    assert_eq!(e404.exit_code(), 69);
    assert!(e404.message.contains("not found"));
    assert!(e404.provider_detail.is_some());
    // a 401 from a gated remote Ollama is an auth failure (exit 77)
    assert_eq!(err(401).kind, ErrorKind::Auth);
}

#[test]
fn mid_stream_error_line_is_a_transport_error() {
    let ev = one_line("{\"error\":\"llama runner process has terminated\"}");
    match &ev[..] {
        [Event::Error(e)] => {
            assert_eq!(e.kind, ErrorKind::Transport); // no governing status (§5.9)
            assert_eq!(e.exit_code(), 69);
            assert!(e.provider_detail.is_none());
        }
        other => panic!("expected one Error, got {other:?}"),
    }
}

#[test]
fn malformed_line_surfaces_a_transport_error() {
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        status: None,
    };
    let err = OllamaChat
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}
