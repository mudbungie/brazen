//! `bz --serve` connection behavior over the in-memory `Bind`/`Listener` seams
//! (ingress.md §7, §14): both response shapes on the data route, keep-alive
//! serial requests, `Connection: close`, the bearer gate (401 / admitted),
//! client API keys ignored, and the mid-stream client disconnect killing only
//! its own connection's upstream. Pseudo-routes live in `serve_routes`, the
//! pre-loop fatals in `serve_control`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::testing::{MemConn, MemoryModelCache, MockTransport};
use crate::tests::run_support::BASIC;
use crate::tests::serve_support::*;
use crate::{CanonicalError, Transport, TransportResponse, WireRequest};

#[test]
fn the_data_route_answers_the_aggregate_shape_with_content_length() {
    let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, ""));
    let cfg = masq_cfg("");
    let (code, _, err) = drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 0, "{err}");
    let out = wrote_str(&wrote);
    assert!(
        out.starts_with("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: "),
        "{out}"
    );
    assert!(out.contains(r#""object":"chat.completion""#), "{out}");
    assert!(out.contains(r#""content":"Hello""#), "{out}");
}

#[test]
fn stream_true_answers_chunked_sse() {
    let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", SSE, ""));
    let cfg = masq_cfg("");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\n\r\n"), "{out}");
    assert!(out.contains(r#"data: {"choices""#), "{out}");
    assert!(out.contains("data: [DONE]\n\n"), "{out}");
    assert!(
        out.ends_with("0\r\n\r\n"),
        "the chunked terminator closes the response: {out}"
    );
}

#[test]
fn keep_alive_serves_serial_requests_and_connection_close_ends_it() {
    // TWO requests on one connection → two responses in order (§7).
    let two = [
        post("/v1/chat/completions", AGG, ""),
        post("/v1/chat/completions", AGG, ""),
    ]
    .concat();
    let (conn, wrote) = MemConn::new(&two);
    let cfg = masq_cfg("");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert_eq!(out.matches("HTTP/1.1 200 OK").count(), 2, "{out}");

    // `Connection: close` after the FIRST request: the second is never read.
    let two = [
        post("/v1/chat/completions", AGG, "connection: close\r\n"),
        post("/v1/chat/completions", AGG, ""),
    ]
    .concat();
    let (conn, wrote) = MemConn::new(&two);
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(wrote_str(&wrote).matches("HTTP/1.1 200 OK").count(), 1);
}

#[test]
fn the_bearer_gate_401s_missing_and_wrong_tokens_and_admits_the_right_one() {
    let cfg = masq_cfg("token = \"hunter2\"\n");
    for headers in [
        "",
        "authorization: Bearer wrong\r\n",
        "authorization: bearer hunter2\r\n",
    ] {
        let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, headers));
        drive(
            &cfg,
            vec![Box::new(conn)],
            &MockTransport::ok(vec![BASIC]),
            &MemoryModelCache::new(),
        );
        let out = wrote_str(&wrote);
        assert!(
            out.starts_with("HTTP/1.1 401 Unauthorized"),
            "{headers}: {out}"
        );
        assert!(out.contains(r#""type":"authentication_error""#), "{out}");
    }
    let (conn, wrote) = MemConn::new(&post(
        "/v1/chat/completions",
        AGG,
        "authorization: Bearer hunter2\r\n",
    ));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert!(wrote_str(&wrote).starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn client_api_keys_are_ignored_and_upstream_auth_is_brazens_own() {
    // The client's fictional key rides Authorization when NO token gates the
    // door; the upstream wire still carries the row's own credential (§7).
    let tx = MockTransport::ok(vec![BASIC]);
    let (conn, wrote) = MemConn::new(&post(
        "/v1/chat/completions",
        AGG,
        "authorization: Bearer sk-client-fiction\r\n",
    ));
    let cfg = masq_cfg("");
    drive(&cfg, vec![Box::new(conn)], &tx, &MemoryModelCache::new());
    assert!(wrote_str(&wrote).starts_with("HTTP/1.1 200 OK"));
    let sent = tx.requests();
    let key = sent[0]
        .headers
        .iter()
        .find(|(n, _)| n == "x-api-key")
        .map(|(_, v)| v.clone());
    assert_eq!(
        key.as_deref(),
        Some("sk-test"),
        "row auth, not the client's"
    );
}

/// A transport whose streamed body counts every chunk the pipeline PULLS —
/// the observable half of "a client disconnect drops the upstream read" (§7).
struct CountingTransport {
    pulls: Arc<AtomicUsize>,
}

impl Transport for CountingTransport {
    fn send(&self, _: WireRequest) -> Result<TransportResponse, CanonicalError> {
        let pulls = Arc::clone(&self.pulls);
        // The fixture split into MANY chunks, each pull counted.
        let chunks: Vec<Vec<u8>> = BASIC.chunks(64).map(<[u8]>::to_vec).collect();
        let body = chunks.into_iter().map(move |c| {
            pulls.fetch_add(1, Ordering::SeqCst);
            Ok(c)
        });
        Ok(TransportResponse {
            status: 200,
            body: Box::new(body),
            retry_after: None,
        })
    }
}

#[test]
fn a_mid_stream_disconnect_kills_only_that_connections_upstream() {
    let cfg = masq_cfg("");
    let pulls = Arc::new(AtomicUsize::new(0));
    let tx = CountingTransport {
        pulls: Arc::clone(&pulls),
    };
    let total = BASIC.chunks(64).count();
    // Connection 1 accepts the SSE header + one frame, then dies; connection 2
    // is a whole healthy turn — served in full despite its sibling's death.
    let (dying, died) = MemConn::failing_after(&post("/v1/chat/completions", SSE, ""), 220);
    let (healthy, ok) = MemConn::new(&post("/v1/chat/completions", SSE, ""));
    let (code, _, _) = drive(
        &cfg,
        vec![Box::new(dying), Box::new(healthy)],
        &tx,
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 0, "the LOOP survives a dead client");
    assert!(
        wrote_str(&ok).contains("data: [DONE]"),
        "the sibling finished"
    );
    let died_out = wrote_str(&died);
    assert!(
        !died_out.contains("data: [DONE]"),
        "the dead one was cut: {died_out}"
    );
    assert!(
        pulls.load(Ordering::SeqCst) < 2 * total,
        "the dead connection's upstream read was DROPPED, not drained: {} of {}",
        pulls.load(Ordering::SeqCst),
        2 * total
    );
}

#[test]
fn a_client_dead_before_the_aggregate_header_still_only_kills_its_connection() {
    // A zero-budget connection: even the AGGREGATE header write fails. The loop
    // shrugs; nothing panics; nothing was written.
    let cfg = masq_cfg("");
    let (conn, wrote) = MemConn::failing_after(&post("/v1/chat/completions", AGG, ""), 0);
    let (code, _, _) = drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 0);
    assert!(wrote_str(&wrote).is_empty());
}
