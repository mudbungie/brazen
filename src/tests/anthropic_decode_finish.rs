//! `decode` terminal coverage (anthropic-messages §3/§4): every `stop_reason`
//! mapping, the in-stream error-kind table, and the non-JSON-body guard. The
//! block/delta branch coverage lives in `anthropic_decode` — crafted frames, no
//! network. Per-file harness copy (the `dec` helper), like the fixture siblings.

use crate::protocol::anthropic::AnthropicMessages;
use crate::{CanonicalError, DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};
use serde_json::{json, Value};

/// Decode one streamed frame (a normal SSE block payload) against `state`.
fn dec(v: Value, state: &mut DecodeState) -> Vec<Event> {
    let data = serde_json::to_vec(&v).unwrap();
    let frame = Frame {
        event: None,
        data,
        status: None,
    };
    AnthropicMessages.decode(frame, state).unwrap()
}

/// Decode a terminal `message_delta` carrying `delta` and return its `Finish`.
fn finish(delta: Value) -> FinishReason {
    let mut s = DecodeState::default();
    let ev = dec(json!({"type":"message_delta","delta":delta}), &mut s);
    match ev.into_iter().last() {
        Some(Event::Finish { reason }) => reason,
        other => panic!("expected Finish, got {other:?}"),
    }
}

#[test]
fn every_stop_reason_maps_to_its_finish_reason() {
    assert_eq!(
        finish(json!({"stop_reason":"end_turn"})),
        FinishReason::Stop
    );
    assert_eq!(
        finish(json!({"stop_reason":"max_tokens"})),
        FinishReason::Length
    );
    assert_eq!(
        finish(json!({"stop_reason":"stop_sequence"})),
        FinishReason::StopSequence
    );
    assert_eq!(
        finish(json!({"stop_reason":"tool_use"})),
        FinishReason::ToolUse
    );
    assert_eq!(
        finish(json!({"stop_reason":"pause_turn"})),
        FinishReason::Pause
    );
    assert_eq!(
        finish(json!({"stop_reason":"model_context_window_exceeded"})),
        FinishReason::Other("model_context_window_exceeded".into())
    );
}

#[test]
fn refusal_reads_stop_details_and_tolerates_their_absence() {
    assert_eq!(
        finish(json!({"stop_reason":"refusal",
                      "stop_details":{"category":"bio","explanation":"no"}})),
        FinishReason::Refusal {
            category: "bio".into(),
            explanation: Some("no".into())
        }
    );
    // CR-A8: an in-stream refusal without stop_details degrades to empty/None, still a Finish.
    assert_eq!(
        finish(json!({"stop_reason":"refusal"})),
        FinishReason::Refusal {
            category: String::new(),
            explanation: None
        }
    );
}

/// The `kind` a one-frame `error` envelope of the given `error.type` decodes to.
fn err_kind(t: &str) -> ErrorKind {
    let mut s = DecodeState::default();
    match dec(
        json!({"type":"error","error":{"type":t,"message":"m"}}),
        &mut s,
    )
    .remove(0)
    {
        Event::Error(e) => e.kind,
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn error_type_table_maps_status_and_class() {
    use ErrorKind::Provider;
    assert_eq!(err_kind("authentication_error"), ErrorKind::Auth);
    assert_eq!(err_kind("permission_error"), ErrorKind::Auth);
    assert_eq!(err_kind("invalid_request_error"), Provider { status: 400 });
    assert_eq!(err_kind("billing_error"), Provider { status: 402 });
    assert_eq!(err_kind("not_found_error"), Provider { status: 404 });
    assert_eq!(err_kind("request_too_large"), Provider { status: 413 });
    assert_eq!(err_kind("rate_limit_error"), Provider { status: 429 });
    assert_eq!(err_kind("api_error"), Provider { status: 500 });
    assert_eq!(err_kind("timeout_error"), Provider { status: 504 });
    assert_eq!(err_kind("overloaded_error"), Provider { status: 529 });
    assert_eq!(err_kind("something_new"), ErrorKind::Transport);
}

/// A non-JSON body is `Transport` ONLY when no status governs (§4.2). With a
/// carried status the status is authoritative (§4.3): a 5xx with proxy HTML still
/// yields `Provider{502}` (exit 70) — never Transport — body surfaced (bl-5fe6).
#[test]
fn non_json_body_is_transport_only_without_a_governing_status() {
    let dec = |status| {
        let frame = Frame {
            event: None,
            data: b"<html>502</html>".to_vec(),
            status,
        };
        AnthropicMessages.decode(frame, &mut DecodeState::default())
    };
    let transport: CanonicalError = dec(None).unwrap_err();
    assert_eq!(transport.kind, ErrorKind::Transport);
    assert!(transport.retryable());
    let Some(Event::Error(e)) = dec(Some(502)).unwrap().pop() else {
        panic!("expected Error");
    };
    assert_eq!(e.kind, ErrorKind::Provider { status: 502 });
    assert_eq!(e.exit_code(), 70);
    assert!(e.message == "<html>502</html>" && e.provider_detail.is_some());
}
