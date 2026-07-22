//! `claude_code` RESPONSE projection unit arms (claude-code spec §5, §6): the line
//! dispatch, every `result` fold arm, the carried `assistant` error tag, the
//! delegated whole-body status frame, and `decode_full`'s line replay. The real
//! captured streams are in `claude_code_fixtures.rs`; these are the targeted arms.

use serde_json::json;

use crate::protocol::claude_code::ClaudeCode;
use crate::{DecodeState, ErrorKind, Event, Frame, Protocol};

fn line(v: serde_json::Value) -> Frame {
    Frame {
        event: None,
        data: v.to_string().into_bytes(),
        status: None,
    }
}

/// A minimal `result` line; `is_error`/`api_error_status`/`result` shaped per test.
fn result_line(is_error: bool, status: Option<u16>, text: Option<&str>) -> Frame {
    line(json!({
        "type": "result",
        "subtype": "success", // the REAL logged-out capture says success even on error
        "is_error": is_error,
        "api_error_status": status,
        "result": text,
    }))
}

#[test]
fn envelope_chatter_yields_nothing() {
    // `system`/`rate_limit_event`/unknown lines are envelope, not content (spec §5.2).
    let mut st = DecodeState::default();
    for v in [
        json!({"type": "system", "subtype": "init"}),
        json!({"type": "rate_limit_event"}),
        json!({"type": "prompt_suggestion"}),
    ] {
        assert_eq!(ClaudeCode.decode(line(v), &mut st).unwrap(), vec![]);
    }
    assert!(!st.terminated);
}

#[test]
fn a_stream_event_delegates_to_the_messages_decoder() {
    // The wrapped payload IS a Messages SSE event (spec §5.2): message_stop inside
    // terminates through the ONE existing state machine.
    let mut st = DecodeState::default();
    let ev = ClaudeCode
        .decode(
            line(json!({"type": "stream_event", "event": {"type": "message_stop"}})),
            &mut st,
        )
        .unwrap();
    assert_eq!(ev, vec![]);
    assert!(st.terminated);
}

#[test]
fn a_result_after_the_message_adds_nothing() {
    let mut st = DecodeState {
        terminated: true, // the inner message_stop already ran
        ..Default::default()
    };
    let ev = ClaudeCode
        .decode(result_line(false, None, Some("pong")), &mut st)
        .unwrap();
    assert_eq!(ev, vec![]);
    assert!(st.terminated);
}

#[test]
fn a_result_without_a_message_stream_is_a_transport_error() {
    // A "successful" run that never streamed a message is malformed, never a silent
    // empty exit-0 (spec §5.2 — the non-stream sibling of the §3.2 verdict guard).
    let mut st = DecodeState::default();
    let ev = ClaudeCode
        .decode(result_line(false, None, Some("")), &mut st)
        .unwrap();
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Transport);
    assert!(e.message.contains("without a message stream"));
    assert!(st.terminated); // EOF after it is clean — no premature-EOF pile-on
}

#[test]
fn an_api_error_status_drives_the_kind_through_the_shared_table() {
    // Carried fact #1 (spec §6): the CLI relays the upstream HTTP status.
    let mut st = DecodeState::default();
    let ev = ClaudeCode
        .decode(result_line(true, Some(529), Some("overloaded")), &mut st)
        .unwrap();
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Provider { status: 529 });
    assert_eq!(e.message, "overloaded");
    assert!(e.provider_detail.is_some()); // the whole result object, verbatim
    assert_eq!(e.retry_after_seconds, None); // no handshake header exists — never fabricated
}

#[test]
fn the_assistant_error_tag_classifies_auth() {
    // Carried fact #2 (spec §6): the failed run's `assistant` line tags the class;
    // the terminal `result` fold reads the note — never message-string sniffing.
    let mut st = DecodeState::default();
    let ev = ClaudeCode
        .decode(
            line(json!({"type": "assistant", "error": "authentication_failed"})),
            &mut st,
        )
        .unwrap();
    assert_eq!(ev, vec![]);
    assert_eq!(st.error_tag.as_deref(), Some("authentication_failed"));
    let ev = ClaudeCode
        .decode(result_line(true, None, Some("Not logged in")), &mut st)
        .unwrap();
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Auth);
}

#[test]
fn a_bare_error_defaults_to_transport_with_a_stand_in_message() {
    // No status, no tag, no result text: the response-side safe default (spec §6).
    let mut st = DecodeState::default();
    let ev = ClaudeCode
        .decode(result_line(true, None, None), &mut st)
        .unwrap();
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Transport);
    assert_eq!(e.message, "claude exited with an error");
}

#[test]
fn a_whole_body_status_frame_delegates_to_the_shared_fold() {
    // Unreachable from the shipped exec transport (status is always 200, spec §3.2)
    // but reachable through the seam — kept uniform with every dialect.
    let mut st = DecodeState::default();
    let frame = Frame {
        event: None,
        data: br#"{"error":{"type":"overloaded_error","message":"busy"}}"#.to_vec(),
        status: Some(529),
    };
    let ev = ClaudeCode.decode(frame, &mut st).unwrap();
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Provider { status: 529 });
}

#[test]
fn a_malformed_line_surfaces_as_the_shared_parse_error() {
    let mut st = DecodeState::default();
    let frame = Frame {
        event: None,
        data: b"not json".to_vec(),
        status: None,
    };
    assert!(ClaudeCode.decode(frame, &mut st).is_err());
}

#[test]
fn decode_full_propagates_a_malformed_line() {
    let mut st = DecodeState::default();
    assert!(ClaudeCode.decode_full(b"not json\n", &mut st).is_err());
}

#[test]
fn decode_full_replays_the_lines_and_skips_blanks() {
    // The stream:false fold (spec §5.3): the aggregate IS the stream's lines.
    let body = format!(
        "{}\n\n{}\n",
        json!({"type": "stream_event", "event": {"type": "message_stop"}}),
        json!({"type": "result", "is_error": true, "api_error_status": null,
               "result": "boom"}),
    );
    let mut st = DecodeState::default();
    let ev = ClaudeCode.decode_full(body.as_bytes(), &mut st).unwrap();
    // message_stop terminated first, so the error result still folds (is_error wins
    // over the had-message check) — one in-band error, terminated stays true.
    let [Event::Error(e)] = ev.as_slice() else {
        panic!("expected one error, got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Transport);
    assert!(st.terminated);
}
