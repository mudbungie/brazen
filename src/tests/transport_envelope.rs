//! The stdio HTTP envelope codec (transport spec §5) — the pure half of the
//! operator-selectable transport: what a delegate is asked to perform, and what
//! Brazen reads back. Table tests over the bytes; the spawn side is proven
//! end-to-end by `tests/transport_conformance.rs`.

use crate::{envelope_error, envelope_head, envelope_request, ErrorKind, WireRequest};

fn post() -> WireRequest {
    let mut wire = WireRequest::new("https://api.example.com/v1/messages", b"{\"a\":1}".to_vec());
    wire.set_header("x-api-key", "sk-secret");
    wire.set_header("content-type", "application/json");
    wire
}

#[test]
fn the_request_is_absolute_form_with_headers_verbatim_and_in_order() {
    // Spec §5.1: absolute-form target (what a proxy receives), `wire.headers` in
    // order with nothing synthesized — no Host, no Content-Length, no User-Agent,
    // no Accept-Encoding. Generating those IS the delegated identity.
    let rendered = String::from_utf8(envelope_request(&post())).unwrap();
    assert_eq!(
        rendered,
        "POST https://api.example.com/v1/messages HTTP/1.1\r\n\
         x-api-key: sk-secret\r\n\
         content-type: application/json\r\n\
         \r\n\
         {\"a\":1}"
    );
    let lower = rendered.to_lowercase();
    for synthesized in [
        "host:",
        "content-length:",
        "user-agent:",
        "accept-encoding:",
    ] {
        assert!(!lower.contains(synthesized), "synthesized {synthesized}");
    }
}

#[test]
fn a_get_renders_the_verb_and_an_empty_body() {
    // The `--list-models` GET is the same path with an empty body — no special case.
    let rendered = String::from_utf8(envelope_request(&WireRequest::get(
        "https://api.example.com/v1/models".to_owned(),
    )))
    .unwrap();
    assert_eq!(
        rendered,
        "GET https://api.example.com/v1/models HTTP/1.1\r\n\r\n"
    );
}

#[test]
fn a_head_is_none_until_the_blank_line_arrives() {
    assert!(envelope_head(b"").unwrap().is_none());
    assert!(envelope_head(b"HTTP/1.1 200 OK\r\n").unwrap().is_none());
    assert!(envelope_head(b"HTTP/1.1 200 OK\r\nx: y\r\n")
        .unwrap()
        .is_none());
}

#[test]
fn the_head_yields_status_retry_after_and_where_the_body_starts() {
    let head = envelope_head(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 5\r\n\r\nnope")
        .unwrap()
        .unwrap();
    assert_eq!(head.status, 429);
    // Verbatim (arch §3.3) — the parse to seconds is the pure lib's, later.
    assert_eq!(head.retry_after.as_deref(), Some("5"));
    // Body bytes that rode along with the head stream on as the FIRST chunk.
    assert_eq!(
        head.body_start,
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 5\r\n\r\n".len()
    );
}

#[test]
fn a_bare_lf_head_and_an_http2_status_line_both_parse() {
    // Lenient in what it accepts: the delegate is the operator's own program. A
    // bare-LF head, and any `HTTP/x` spelling — the status is the second token.
    let head = envelope_head(b"HTTP/2 200\nx-thing: 1\n\nbody")
        .unwrap()
        .unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(head.retry_after, None);
    assert_eq!(
        &b"HTTP/2 200\nx-thing: 1\n\nbody"[head.body_start..],
        b"body"
    );
}

#[test]
fn a_head_with_no_body_bytes_yet_starts_the_body_at_the_end() {
    let raw = b"HTTP/1.1 200 OK\r\n\r\n";
    let head = envelope_head(raw).unwrap().unwrap();
    assert_eq!(head.status, 200);
    assert_eq!(head.body_start, raw.len());
}

#[test]
fn a_missing_status_line_is_a_transport_error_that_echoes_nothing() {
    // Spec §6: a malformed envelope names a REASON and never echoes head bytes — a
    // delegate that echoed the request back must not make `bz` print the credential.
    let err =
        envelope_head(b"garbage from a broken relay\r\nx-api-key: sk-secret\r\n\r\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
    assert!(
        err.message.contains("no HTTP status line"),
        "{}",
        err.message
    );
    assert!(!err.message.contains("sk-secret"), "{}", err.message);
}

#[test]
fn a_non_numeric_status_token_is_a_transport_error() {
    let err = envelope_head(b"HTTP/1.1 OK\r\n\r\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
    assert!(err.message.contains("no HTTP status line"));
}

#[test]
fn an_unterminated_head_past_64_kib_is_a_transport_error() {
    // The silence budget catches a STALLED delegate, not a chatty one: without this
    // ceiling a delegate that never emits the blank line buffers without bound.
    let mut buf = b"HTTP/1.1 200 OK\r\n".to_vec();
    buf.extend(std::iter::repeat_n(b'x', 64 * 1024 + 1));
    let err = envelope_head(&buf).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
    assert!(err.message.contains("64 KiB"), "{}", err.message);
}

#[test]
fn every_envelope_error_is_a_transport_kind_with_no_provider_detail() {
    // There is no upstream response to carry: the delegate failed BELOW the protocol.
    let err = envelope_error("boom");
    assert_eq!(err.kind, ErrorKind::Transport);
    assert_eq!(err.message, "stdio transport: boom");
    assert!(err.provider_detail.is_none());
    assert!(err.retry_after_seconds.is_none());
}
