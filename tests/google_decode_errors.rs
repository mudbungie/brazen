//! Decode coverage for `google_generative_ai` arms not reached by a full fixture
//! (providers §4.4/§4.7/§4.8): the `finishReason` variants (length/safety/unknown),
//! a mid-stream `usageMetadata`-only chunk, a finish chunk with no usage, the nested
//! error envelope, and a malformed chunk → `Transport`. No network.

use brazen::protocol::google_genai::GoogleGenAi;
use brazen::{CanonicalError, DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};

const ERR: &[u8] = include_bytes!("fixtures/google_error.json");

fn one_chunk(json: &str) -> Vec<Event> {
    let frame = Frame {
        event: None,
        data: json.as_bytes().to_vec(),
        status: None,
    };
    GoogleGenAi
        .decode(frame, &mut DecodeState::default())
        .unwrap()
}

/// The `Finish` reason produced by a chunk carrying `finishReason` and no content.
fn finish_of(reason: &str) -> FinishReason {
    let json = format!(
        "{{\"candidates\":[{{\"content\":{{\"parts\":[]}},\"finishReason\":\"{reason}\"}}]}}"
    );
    match one_chunk(&json)
        .into_iter()
        .find(|e| matches!(e, Event::Finish { .. }))
    {
        Some(Event::Finish { reason }) => reason,
        other => panic!("expected Finish, got {other:?}"),
    }
}

#[test]
fn finish_reason_maps_length_safety_and_unknown() {
    assert_eq!(finish_of("MAX_TOKENS"), FinishReason::Length);
    assert_eq!(
        finish_of("SAFETY"),
        FinishReason::Refusal {
            category: "safety".into(), // HTTP 200, exit 0 (§4.7)
            explanation: None,
        }
    );
    assert_eq!(
        finish_of("RECITATION"),
        FinishReason::Other("RECITATION".into())
    );
}

#[test]
fn finish_chunk_without_usage_emits_no_usage_event() {
    let ev = one_chunk("{\"candidates\":[{\"content\":{\"parts\":[]},\"finishReason\":\"STOP\"}]}");
    assert!(!ev.iter().any(|e| matches!(e, Event::Usage(_))));
    assert!(matches!(
        ev.last(),
        Some(Event::Finish {
            reason: FinishReason::Stop
        })
    ));
}

#[test]
fn mid_stream_usage_only_chunk_emits_usage_without_finish() {
    let ev = one_chunk("{\"usageMetadata\":{\"promptTokenCount\":7}}");
    assert!(ev.iter().any(|e| matches!(e, Event::Usage(_))));
    assert!(!ev.iter().any(|e| matches!(e, Event::Finish { .. })));
}

#[test]
fn whole_body_error_maps_the_status_family() {
    let err = |status: u16| -> CanonicalError {
        let frame = Frame {
            event: None,
            data: ERR.to_vec(),
            status: Some(status),
        };
        match GoogleGenAi
            .decode(frame, &mut DecodeState::default())
            .unwrap()
            .pop()
        {
            Some(Event::Error(e)) => e,
            other => panic!("expected Error, got {other:?}"),
        }
    };
    let e429 = err(429);
    assert_eq!(e429.kind, ErrorKind::Provider { status: 429 });
    assert_eq!(e429.exit_code(), 69);
    assert!(e429.retryable());
    assert!(e429.message.contains("exhausted"));
    assert!(e429.provider_detail.is_some());
    assert_eq!(err(403).kind, ErrorKind::Auth); // PERMISSION_DENIED → 403 → 77
}

#[test]
fn malformed_chunk_surfaces_a_transport_error() {
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        status: None,
    };
    let err = GoogleGenAi
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}
