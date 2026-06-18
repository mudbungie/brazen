//! Decode coverage for `openai_responses` completion / error arms not reached by a
//! full fixture (providers §3.6/§3.7): the reasoning-summary refusal channel,
//! incomplete/unknown finish reasons, the mid-stream + whole-body error envelopes,
//! the keep-alive/unknown event, completion's open-block drain, and a malformed
//! frame. The block-routing arms live in the sibling `responses_decode_errors`. No
//! network.

mod responses_decode_errors_support;
use responses_decode_errors_support::{finish_of, run, CREATED, ERR_401};

use brazen::protocol::openai_responses::OpenAiResponses;
use brazen::{CanonicalError, DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};

#[test]
fn a_streamed_refusal_wins_at_completion() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.refusal.delta","output_index":0,"delta":"I can't help with that."}"#,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert_eq!(
        finish_of(&ev),
        &FinishReason::Refusal {
            category: "refusal".into(),
            explanation: Some("I can't help with that.".into()),
        }
    );
}

#[test]
fn an_unknown_completed_status_is_preserved_verbatim() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.completed","response":{"status":"expired","output":[]}}"#,
    ]);
    assert_eq!(finish_of(&ev), &FinishReason::Other("expired".into()));
}

#[test]
fn completed_without_usage_emits_no_usage_event() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::Usage(_))));
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}

#[test]
fn incomplete_maps_length_and_other() {
    let length = run(&[
        CREATED,
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
    ]);
    assert_eq!(finish_of(&length), &FinishReason::Length);
    let other = run(&[
        CREATED,
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}"#,
    ]);
    assert_eq!(
        finish_of(&other),
        &FinishReason::Other("content_filter".into())
    );
}

#[test]
fn mid_stream_errors_decode_kind_from_the_body() {
    // response.failed: a server fault (`code`, nested under response.error) is
    // 5xx-class → Provider{500}/70, NOT a blanket Transport (§3.7, CR-10).
    let failed = run(&[
        CREATED,
        r#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"the model failed"}}}"#,
    ]);
    match failed.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Provider { status: 500 });
            assert_eq!(e.exit_code(), 70);
            assert_eq!(e.message, "the model failed");
            assert!(e.provider_detail.is_some());
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // response.error at top level: a rate limit (`code`) → Provider{429}/69.
    let limited = run(&[
        r#"{"type":"response.error","error":{"code":"rate_limit_exceeded","message":"slow"}}"#,
    ]);
    match limited.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Provider { status: 429 });
            assert_eq!(e.exit_code(), 69);
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // the tag may ride `type` when `code` is absent (the or_else fallback).
    let typed =
        run(&[r#"{"type":"response.error","error":{"type":"rate_limit_error","message":"slow"}}"#]);
    match typed.last() {
        Some(Event::Error(e)) => assert_eq!(e.kind, ErrorKind::Provider { status: 429 }),
        other => panic!("expected Error, got {other:?}"),
    }
    // an unrecognized/absent tag stays retryable Transport (exit 69).
    let untyped = run(&[r#"{"type":"response.error","error":{"message":"boom"}}"#]);
    match untyped.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Transport);
            assert_eq!(e.message, "boom");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn whole_body_error_maps_the_status_family() {
    let frame = Frame {
        event: None,
        data: ERR_401.to_vec(),
        status: Some(401),
    };
    match OpenAiResponses
        .decode(frame, &mut DecodeState::default())
        .unwrap()
        .pop()
    {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Auth);
            assert_eq!(e.exit_code(), 77);
            assert_eq!(e.message, "Incorrect API key provided.");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn an_unknown_event_type_yields_nothing() {
    assert!(run(&[r#"{"type":"response.queued"}"#]).is_empty()); // keep-alive / future type
}

#[test]
fn completion_drains_a_still_open_block() {
    // no output_item.done arrives, so response.completed drains the open text block
    let ev = run(&[
        CREATED,
        r#"{"type":"response.content_part.added","output_index":0,"part":{"type":"output_text"}}"#,
        r#"{"type":"response.output_text.delta","output_index":0,"delta":"hi"}"#,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert!(ev
        .iter()
        .any(|e| matches!(e, Event::ContentStop { index: 0 })));
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}

#[test]
fn malformed_frame_surfaces_a_transport_error() {
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        status: None,
    };
    let err: CanonicalError = OpenAiResponses
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}
