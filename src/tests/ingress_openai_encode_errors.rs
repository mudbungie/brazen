//! The §9 error masquerade (ingress.md, tested per §14): the carried
//! `Provider{status}` fact wins; every statusless kind projects through the
//! shared `ErrorKind` table read in reverse (`http_status` — one table, one
//! module); the OpenAI envelope carries the status as its NUMERIC `code` so the
//! forward decoder's §4.3 proxy convention reads the kind back losslessly;
//! mid-stream = an error chunk then stream end; the aggregate error IS the body
//! and `IngressState::status` is the listener's HTTP join point.

use serde_json::json;

use super::ingress_encode_support::{
    agg_req, encode_all, ms, state, stream_req, text_delta, text_start,
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
fn a_carried_status_masquerades_mid_stream() {
    // The upstream fact rides Provider{status} — never re-derived (§9). On the
    // SSE shape the envelope is its own chunk and End's [DONE] closes the
    // stream, the mid-stream convention this dialect's SDKs tolerate.
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        text_start(0),
        text_delta(0, "half"),
        err(ErrorKind::Provider { status: 503 }, "upstream overloaded"),
        Event::End,
    ];
    let sse = encode_all(&events, &mut st);
    assert!(sse.ends_with(concat!(
        "data: {\"error\":{\"code\":503,\"message\":\"upstream overloaded\",",
        "\"param\":null,\"type\":\"server_error\"}}\n\n",
        "data: [DONE]\n\n",
    )));
    assert_eq!(st.status(), 503);
}

#[test]
fn the_masqueraded_error_decodes_back_through_the_shared_table() {
    // Round trip: the numeric `code` is the §4.3 proxy convention the EGRESS
    // decoder reads through the one shared from_http_status table.
    let mut st = state(stream_req(), &[]);
    let sse = encode_all(
        &[
            err(ErrorKind::Provider { status: 429 }, "slow down"),
            Event::End,
        ],
        &mut st,
    );
    let (events, _) = super::ingress_encode_support::egress_decode(&sse);
    let Event::Error(e) = &events[0] else {
        panic!("not an error: {events:?}");
    };
    assert_eq!(e.kind, ErrorKind::Provider { status: 429 });
    assert_eq!(e.message, "slow down");
}

#[test]
fn statusless_kinds_project_through_the_reverse_table() {
    // The shared table read in reverse (§9): kind → status → the dialect's
    // error `type` vocabulary, so client retry logic keeps working.
    let cases = [
        (ErrorKind::Auth, 401, "authentication_error"),
        (ErrorKind::ParseInput, 400, "invalid_request_error"),
        (ErrorKind::Usage, 400, "invalid_request_error"),
        (
            ErrorKind::Provider { status: 404 },
            404,
            "invalid_request_error",
        ),
        (ErrorKind::Provider { status: 429 }, 429, "rate_limit_error"),
        (ErrorKind::Transport, 502, "server_error"),
        (ErrorKind::Config, 500, "server_error"),
        (ErrorKind::Interrupted, 500, "server_error"),
        (ErrorKind::Other("mystery".into()), 500, "server_error"),
    ];
    for (kind, status, ty) in cases {
        let mut st = state(agg_req(), &[]);
        let body = encode_all(&[err(kind.clone(), "boom"), Event::End], &mut st);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], json!(status), "{kind:?}");
        assert_eq!(v["error"]["type"], json!(ty), "{kind:?}");
        assert_eq!(v["error"]["message"], json!("boom"));
        assert_eq!(st.status(), status, "{kind:?}");
    }
}

#[test]
fn an_aggregate_error_is_the_whole_body() {
    // The fold is discarded: the §9 envelope replaces the success body, and the
    // §4 exposure still rides it (the adaptation DID fire on this request).
    let mut st = state(agg_req(), &["thinking_replay"]);
    let events = [
        ms(),
        text_start(0),
        text_delta(0, "half a reply"),
        err(ErrorKind::Auth, "key rejected"),
        Event::End,
    ];
    assert_eq!(
        encode_all(&events, &mut st),
        concat!(
            "{\"brazen\":{\"adaptations\":[\"thinking_replay\"]},",
            "\"error\":{\"code\":401,\"message\":\"key rejected\",",
            "\"param\":null,\"type\":\"authentication_error\"}}",
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
