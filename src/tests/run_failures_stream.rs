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
