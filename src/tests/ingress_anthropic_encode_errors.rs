//! The §9 error masquerade for `anthropic_messages` (ingress.md, tested per §14): the
//! carried `Provider{status}` fact drives the HTTP status (`IngressState::status`); the
//! in-band envelope is Anthropic's `{"type":"error","error":{"type","message"}}` with a
//! COARSE `error.type` FAMILY (the anthropic wire has no numeric status slot — the
//! documented narrowing). Mid-stream = an `event: error` frame then stream end (no
//! message_stop); the aggregate error IS the body.

use serde_json::json;

use super::ingress_anthropic_support::{
    agg_req, egress_decode, encode_all, ms, payloads, state, stream_req,
};
use crate::{CanonicalError, ErrorKind, Event};

fn err(kind: ErrorKind, msg: &str) -> Event {
    Event::Error(CanonicalError {
        kind,
        message: msg.into(),
        provider_detail: None,
        retry_after_seconds: None,
    })
}

#[test]
fn a_mid_stream_error_frames_then_ends_without_message_stop() {
    // §3.8 inverted: the error is its own `event: error` frame and the stream closes
    // after it — End emits NO message_stop (the anthropic mid-stream convention).
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        err(ErrorKind::Provider { status: 529 }, "Overloaded"),
        Event::End,
    ];
    let sse = encode_all(&events, &mut st);
    assert!(sse.contains(
        "event: error\ndata: {\"error\":{\"message\":\"Overloaded\",\"type\":\"overloaded_error\"},\"type\":\"error\"}\n\n"
    ));
    assert!(!sse.contains("message_stop")); // terminal error: no message_stop follows
    assert_eq!(st.status(), 529);
}

#[test]
fn the_masqueraded_error_decodes_back_through_the_type_family() {
    // Round trip on the families that map losslessly: the EGRESS decoder reads error.type
    // back to the same kind for auth / rate-limit / api classes.
    for (kind, msg) in [
        (ErrorKind::Provider { status: 429 }, "slow down"),
        (ErrorKind::Auth, "key rejected"),
        (ErrorKind::Provider { status: 500 }, "boom"),
    ] {
        let mut st = state(stream_req(), &[]);
        let sse = encode_all(&[err(kind.clone(), msg), Event::End], &mut st);
        let (events, _) = egress_decode(&sse);
        let Event::Error(e) = &events[0] else {
            panic!("not an error: {events:?}");
        };
        assert_eq!(e.kind, kind);
        assert_eq!(e.message, msg);
    }
}

#[test]
fn a_specific_status_coarsens_to_its_family_the_documented_narrowing() {
    // The narrowing (§9): Anthropic's envelope has no numeric status, so a 503 projects
    // to `api_error` and re-decodes to 500 — the precise status survives only on the HTTP
    // layer (`status()`), never the in-band `error.type`.
    let mut st = state(stream_req(), &[]);
    let sse = encode_all(
        &[err(ErrorKind::Provider { status: 503 }, "x"), Event::End],
        &mut st,
    );
    assert_eq!(st.status(), 503); // HTTP layer keeps the precise status
    let (events, _) = egress_decode(&sse);
    let Event::Error(e) = &events[0] else {
        panic!();
    };
    assert_eq!(e.kind, ErrorKind::Provider { status: 500 }); // in-band coarsens to the family
}

#[test]
fn the_error_type_family_projects_from_the_status() {
    // The reverse projection (§9): every kind → its status → the anthropic error.type
    // FAMILY, so client retry logic keeps working.
    let cases = [
        (ErrorKind::Auth, 401, "authentication_error"),
        (
            ErrorKind::Provider { status: 403 },
            403,
            "authentication_error",
        ),
        (ErrorKind::ParseInput, 400, "invalid_request_error"),
        (ErrorKind::Usage, 400, "invalid_request_error"),
        (ErrorKind::Provider { status: 402 }, 402, "billing_error"),
        (ErrorKind::Provider { status: 404 }, 404, "not_found_error"),
        (
            ErrorKind::Provider { status: 413 },
            413,
            "request_too_large",
        ),
        (ErrorKind::Provider { status: 429 }, 429, "rate_limit_error"),
        (ErrorKind::Provider { status: 504 }, 504, "timeout_error"),
        (ErrorKind::Provider { status: 529 }, 529, "overloaded_error"),
        (ErrorKind::Provider { status: 500 }, 500, "api_error"),
        (ErrorKind::Transport, 502, "api_error"),
        (ErrorKind::Config, 500, "api_error"),
        (ErrorKind::Interrupted, 500, "api_error"),
        (ErrorKind::Other("mystery".into()), 500, "api_error"),
        (
            ErrorKind::Provider { status: 418 },
            418,
            "invalid_request_error",
        ),
    ];
    for (kind, status, ty) in cases {
        let mut st = state(agg_req(), &[]);
        let body = encode_all(&[err(kind.clone(), "boom"), Event::End], &mut st);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["type"], json!(ty), "{kind:?}");
        assert_eq!(v["error"]["message"], json!("boom"));
        assert_eq!(v["type"], json!("error"));
        assert_eq!(st.status(), status, "{kind:?}");
    }
}

#[test]
fn an_aggregate_error_is_the_whole_body_with_exposure() {
    // The fold is discarded: the §9 envelope replaces the success body, and the §4
    // exposure still rides it.
    let mut st = state(agg_req(), &["thinking_replay"]);
    let events = [ms(), err(ErrorKind::Auth, "key rejected"), Event::End];
    assert_eq!(
        encode_all(&events, &mut st),
        concat!(
            "{\"brazen\":{\"adaptations\":[\"thinking_replay\"]},",
            "\"error\":{\"message\":\"key rejected\",\"type\":\"authentication_error\"},",
            "\"type\":\"error\"}",
        )
    );
    assert_eq!(st.status(), 401);
}

#[test]
fn the_last_error_wins() {
    let mut st = state(agg_req(), &[]);
    let events = [
        err(ErrorKind::Provider { status: 500 }, "first"),
        err(ErrorKind::Provider { status: 429 }, "second"),
        Event::End,
    ];
    let v: serde_json::Value = serde_json::from_str(&encode_all(&events, &mut st)).unwrap();
    assert_eq!(v["error"]["message"], json!("second"));
    assert_eq!(st.status(), 429);
}

#[test]
fn a_streamed_error_with_no_prior_frame_still_frames() {
    // The edge-rejection path (§9): Error then End with no MessageStart — the error frame
    // is the whole stream (still no message_stop).
    let mut st = state(stream_req(), &[]);
    let sse = encode_all(
        &[err(ErrorKind::ParseInput, "bad body"), Event::End],
        &mut st,
    );
    assert_eq!(
        payloads(&sse)[0]["error"]["type"],
        json!("invalid_request_error")
    );
    assert_eq!(st.status(), 400);
}
