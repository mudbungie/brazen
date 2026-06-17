//! Fine-grained `decode` branch coverage (anthropic-messages §3/§4): each
//! `data.type`, each delta kind, every `stop_reason`, the error-kind table, and
//! the malformed/absent-field guards — crafted frames, no network.

use brazen::protocol::anthropic::AnthropicMessages;
use brazen::{
    CanonicalError, ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason, Frame,
    Protocol, Role, Usage,
};
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

#[test]
fn message_start_without_usage_is_just_message_start() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"message_start","message":{"id":"m","model":"x","role":"assistant"}}),
        &mut s,
    );
    assert_eq!(
        ev,
        vec![Event::message_start(
            Some("m".into()),
            Some("x".into()),
            Role::Assistant
        )]
    );
}

#[test]
fn ping_and_unknown_types_emit_nothing() {
    let mut s = DecodeState::default();
    assert_eq!(dec(json!({"type":"ping"}), &mut s), vec![]);
    assert_eq!(dec(json!({"type":"future_event"}), &mut s), vec![]);
    assert!(!s.terminated);
}

#[test]
fn message_stop_sets_terminated_and_emits_nothing() {
    let mut s = DecodeState::default();
    assert_eq!(dec(json!({"type":"message_stop"}), &mut s), vec![]);
    assert!(s.terminated);
}

#[test]
fn redacted_thinking_block_round_trips_data_verbatim() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"content_block_start","index":0,
               "content_block":{"type":"redacted_thinking","data":"OPAQUE=="}}),
        &mut s,
    );
    assert_eq!(
        ev,
        vec![Event::ContentStart {
            index: 0,
            kind: ContentKind::RedactedThinking {}
        }]
    );
    assert_eq!(s.open.get(&0).map(|b| b.buffer.as_str()), Some("OPAQUE=="));
}

#[test]
fn tool_use_json_fragments_accumulate_in_the_buffer() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":1,
               "content_block":{"type":"tool_use","id":"tu","name":"f","input":{}}}),
        &mut s,
    );
    let d1 = dec(
        json!({"type":"content_block_delta","index":1,
               "delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}),
        &mut s,
    );
    dec(
        json!({"type":"content_block_delta","index":1,
               "delta":{"type":"input_json_delta","partial_json":"1}"}}),
        &mut s,
    );
    assert_eq!(
        d1,
        vec![Event::ContentDelta {
            index: 1,
            delta: Delta::JsonDelta("{\"a\":".into())
        }]
    );
    assert_eq!(s.open.get(&1).map(|b| b.buffer.as_str()), Some("{\"a\":1}"));
}

#[test]
fn signature_delta_accumulates_but_emits_no_event() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":0,
               "content_block":{"type":"thinking","thinking":"","signature":""}}),
        &mut s,
    );
    let sig = dec(
        json!({"type":"content_block_delta","index":0,
               "delta":{"type":"signature_delta","signature":"SIG=="}}),
        &mut s,
    );
    assert_eq!(sig, vec![]); // not a Delta (CR-5)
    assert_eq!(s.open.get(&0).map(|b| b.buffer.as_str()), Some("SIG=="));
}

#[test]
fn unknown_delta_on_a_tracked_block_emits_nothing() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        &mut s,
    );
    assert_eq!(
        dec(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"bogus_delta"}}),
            &mut s
        ),
        vec![]
    );
}

#[test]
fn untracked_block_deltas_and_stops_emit_nothing() {
    let mut s = DecodeState::default();
    // server_tool_use has no canonical ContentKind (CR-4): start is dropped, so the
    // index never enters `open` and its delta/stop fall through to [].
    assert_eq!(
        dec(
            json!({"type":"content_block_start","index":3,
                   "content_block":{"type":"server_tool_use","id":"s","name":"web_search"}}),
            &mut s
        ),
        vec![]
    );
    assert_eq!(
        dec(
            json!({"type":"content_block_delta","index":3,
                   "delta":{"type":"input_json_delta","partial_json":"x"}}),
            &mut s
        ),
        vec![]
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop","index":3}), &mut s),
        vec![]
    );
}

#[test]
fn absent_index_defaults_to_zero() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","content_block":{"type":"text","text":""}}),
        &mut s,
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop"}), &mut s),
        vec![Event::ContentStop { index: 0 }]
    );
}

#[test]
fn message_delta_usage_only_emits_no_finish() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":7}}),
        &mut s,
    );
    assert_eq!(
        ev,
        vec![Event::Usage(Usage {
            output: Some(7),
            ..Usage::default()
        })]
    );
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
    // CR-8: an in-stream refusal without stop_details degrades to empty/None, still a Finish.
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
