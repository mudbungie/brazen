//! End-to-end `run` (arch §9.6) — the status→exit table (0/69/70) and in-band vs
//! stderr routing of provider/transport errors. Auth/config errors, the
//! `--dump-config` path, and the SIGPIPE mapping live in `run_config`.

use std::io;

use crate::testing::{Chunk, MockTransport};
use crate::tests::run_support::*;

// ============================ status / exit table ============================

#[test]
fn refusal_is_finish_not_error_exit_0() {
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &MockTransport::ok(vec![REFUSAL]),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains(r#""reason":"refusal""#));
    assert!(!o.stdout.contains(r#""type":"error""#));
}

#[test]
fn whole_body_error_is_in_band_under_json_exit_70() {
    let tx = MockTransport::new(529, vec![Chunk::Data(OVERLOADED.to_vec())]);
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 70);
    assert!(o.stdout.contains(r#""type":"error""#));
    assert!(o.stdout.trim_end().ends_with(r#"{"type":"end"}"#));
}

#[test]
fn whole_body_error_goes_to_stderr_under_text_exit_70() {
    let tx = MockTransport::new(529, vec![Chunk::Data(OVERLOADED.to_vec())]);
    let o = go(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 70);
    assert!(o.stdout.is_empty());
    assert!(o.stderr.contains("Overloaded"));
}

#[test]
fn json_4xx_surfaces_the_raw_provider_body_in_provider_detail() {
    // The bl-5fe6 regression, end to end: a 400 whose body is the codex backend's
    // `{"detail":…}` (NOT a `{"error":…}` envelope). Brazen used to emit message:""
    // / provider_detail:null; now the raw body reaches provider_detail and message,
    // so `bz --json` alone diagnoses the failure.
    let body = br#"{"detail":"Store must be set to false"}"#;
    let tx = MockTransport::new(400, vec![Chunk::Data(body.to_vec())]);
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains(r#""provider":{"status":400}"#));
    assert!(o
        .stdout
        .contains(r#""message":"Store must be set to false""#));
    assert!(o
        .stdout
        .contains(r#""provider_detail":{"detail":"Store must be set to false"}"#));
}

#[test]
fn raw_4xx_streams_body_but_exits_69() {
    let tx = MockTransport::new(400, vec![Chunk::Data(b"upstream error body".to_vec())]);
    let o = go(
        &[
            "--raw",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert_eq!(o.stdout, "upstream error body");
}

#[test]
fn whole_body_drain_drop_is_transport_69() {
    let tx = MockTransport::new(400, vec![Chunk::Fail(io::ErrorKind::ConnectionReset)]);
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("failed to read error response body"));
}

#[test]
fn whole_body_success_drain_drop_is_transport_69() {
    // A 2xx `stream:false` body routes to `whole_body_success`, which drains the
    // aggregate whole (config §4.2). A mid-collection transport drop is the same
    // in-band `Transport` error (69) the error-body drain surfaces — no decode_full.
    let tx = MockTransport::new(200, vec![Chunk::Fail(io::ErrorKind::ConnectionReset)]);
    let req = br#"{"model":"claude-m","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let o = go(
        &["--json", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        req,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("failed to read response body"));
}

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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &ErrTransport,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.contains("connection refused"));
}
