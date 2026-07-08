//! End-to-end `run` (arch §9.6) — the whole-body status→exit table (0/69/70) and
//! in-band vs stderr routing of provider errors. Mid-stream failures (drops,
//! malformed frames, premature EOF) live in `run_failures_stream`; auth/config
//! errors, the `--dump-config` path, and the SIGPIPE mapping in `run_config`.

use std::io;

use crate::testing::{Chunk, MockTransport};
use crate::tests::run_support::*;

#[test]
fn refusal_is_finish_not_error_exit_0() {
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
    assert_eq!(o.code, 70);
    assert!(o.stdout.contains(r#""type":"error""#));
    assert!(o.stdout.trim_end().ends_with(r#"{"type":"end"}"#));
}

#[test]
fn whole_body_error_goes_to_stderr_under_text_exit_70() {
    let tx = MockTransport::new(529, vec![Chunk::Data(OVERLOADED.to_vec())]);
    let o = go(
        &[
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
            "--json",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
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
fn retry_after_header_rides_the_json_error_line_and_is_omitted_when_absent() {
    // bl-135a: a 429 whose response carries `Retry-After: 30` surfaces the
    // transport-level pacing hint as `retry_after_seconds` on the `--json` error line
    // — the fact `provider_detail` (the BODY) never holds, for a caller's retry loop.
    let body = br#"{"type":"error","error":{"type":"rate_limit_error"}}"#;
    let with = MockTransport::new(429, vec![Chunk::Data(body.to_vec())]).with_retry_after("30");
    let hit = go(
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
        &with,
        &empty_store(),
    );
    assert_eq!(hit.code, 69); // 429 → 4xx → 69 (retry policy rides retryable, not the code)
    assert!(hit.stdout.contains(r#""retry_after_seconds":30"#));

    // No header → the key is omitted entirely (additive skip-when-None, so old error
    // lines stay byte-identical); the same 429 is otherwise unchanged.
    let without = MockTransport::new(429, vec![Chunk::Data(body.to_vec())]);
    let miss = go(
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
        &without,
        &empty_store(),
    );
    assert_eq!(miss.code, 69);
    assert!(!miss.stdout.contains("retry_after_seconds"));
}

#[test]
fn whole_body_drain_drop_is_transport_69() {
    let tx = MockTransport::new(400, vec![Chunk::Fail(io::ErrorKind::ConnectionReset)]);
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
