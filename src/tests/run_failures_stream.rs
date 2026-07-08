//! End-to-end `run` (arch §9.6) — mid-stream and handshake failures: a transport
//! drop or malformed frame mid-stream, premature EOF vs the flushed terminator,
//! and a handshake error (all 69, surfaced in-band). The whole-body status→exit
//! table lives in `run_failures`. Driven by `MockTransport`; zero network.

use std::io;

use crate::testing::{Chunk, MockTransport};
use crate::tests::run_support::*;

#[test]
fn transport_drop_mid_stream_is_69() {
    let tx = MockTransport::new(
        200,
        vec![
            Chunk::Data(BASIC[..120].to_vec()),
            Chunk::Fail(io::ErrorKind::ConnectionReset),
        ],
    );
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("transport stream dropped"));
}

#[test]
fn malformed_stream_frame_is_in_band_decode_error() {
    // A streaming frame with invalid JSON: `decode` returns Err, surfaced in-band
    // as an Event::Error (exit 69), then the stream ends premature (also 69).
    const BAD_FRAME: &[u8] = b"event: message_start\ndata: {not valid json}\n\n";
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![BAD_FRAME]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains(r#""type":"error""#));
}

#[test]
fn premature_eof_without_terminator_is_69() {
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![TRUNCATED]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("premature upstream EOF"));
}

/// A 200 whose body is a JSON error (a provider that served its error with the wrong
/// status, or a proxy) never frames — zero frames — so the premature-EOF error carries
/// the body VERBATIM in `provider_detail` rather than discarding it (§5.6). The actual
/// upstream text ("quota exceeded") must be diagnosable, not swallowed by a bare 69.
#[test]
fn non_sse_json_body_served_200_rides_premature_eof() {
    const JSON_200: &[u8] = br#"{"error":{"message":"quota exceeded"}}"#;
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![JSON_200]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("premature upstream EOF"));
    assert!(o.stdout.contains("quota exceeded")); // the body rides provider_detail
}

/// The non-JSON sibling: a gateway HTML page served with 200 also frames nothing, so its
/// bytes ride `provider_detail` as a string (§5.6) — the same care the non-2xx path
/// gives a proxy's HTML body (`json::http_error`).
#[test]
fn non_sse_html_body_served_200_rides_premature_eof() {
    const HTML_200: &[u8] = b"<html><body>502 Bad Gateway</body></html>";
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![HTML_200]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("premature upstream EOF"));
    assert!(o.stdout.contains("502 Bad Gateway"));
}

/// An empty 200 body frames nothing AND has no sample: the premature EOF degrades to the
/// bare error (no `provider_detail`), never fabricating a diagnostic (§5.6).
#[test]
fn empty_200_body_is_bare_premature_eof() {
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("premature upstream EOF"));
    assert!(o.stdout.contains(r#""provider_detail":null"#)); // no sample fabricated
}

/// A clean stream split across two body chunks: the first chunk frames (so the
/// diagnostic head is dropped), the second carries the terminator — proving the head
/// accumulation is a no-op once a frame is seen and never taints a healthy stream.
#[test]
fn clean_stream_across_chunks_frames_and_terminates() {
    const STOP: &[u8] = b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![TRUNCATED, STOP]),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(!o.stdout.contains("premature"));
}

#[test]
fn finish_flushes_trailing_frame_and_terminator_suppresses_premature() {
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![FINISH_FLUSH]),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(!o.stdout.contains("premature"));
    assert!(o.stdout.trim_end().ends_with(r#"{"type":"end"}"#));
}

#[test]
fn transport_handshake_error_is_69() {
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &ErrTransport,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("connection refused"));
}
