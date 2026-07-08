//! `bz --count-tokens` error + decline paths (architecture §5.10.1/§8, providers §10.1):
//! the DECLINE arm (a provider with no count endpoint → Config 78), the status-driven
//! non-2xx fold (77/69/70), a malformed/keyless 2xx body (502→70), a mid-drain drop (69),
//! the input-open/attach failures (66), a malformed/failing request read (64), a stdout
//! write failure (69), and the empty-cache-no-model config error (78). `MockTransport`.

use std::io;

use crate::testing::{Chunk, MemoryCredStore, MockTransport};
use crate::tests::count_support::{go, go_out, go_reader, FailReader, FailWriter, ANT, COUNT, REQ};

#[test]
fn a_provider_with_no_count_endpoint_declines_78() {
    // The DECLINE arm: OpenAI-chat, OpenAI-responses, and Ollama have no count endpoint
    // (the trait default `None`), so the op is the honest Config (78) decline, not a lie.
    for provider in ["openai", "openai-responses", "ollama"] {
        let tx = MockTransport::ok(vec![COUNT]);
        let o = go(
            &["--count-tokens", "--provider", provider, "--api-key", "sk"],
            REQ,
            &tx,
            &MemoryCredStore::new(),
        );
        assert_eq!(o.code, 78, "{provider} declines with Config/78");
        assert!(
            o.stderr.contains("no token-count endpoint"),
            "{provider}: {}",
            o.stderr
        );
        assert!(
            tx.requests().is_empty(),
            "{provider}: declines BEFORE any round-trip"
        );
    }
}

#[test]
fn a_non_2xx_maps_through_http_error() {
    // The count round-trip drained a non-2xx routes through the ONE `http_error` home:
    // 401 → auth 77, other 4xx → 69, 5xx → 70 — the same table the data plane uses.
    for (status, code) in [(401u16, 77u8), (400, 69), (500, 70)] {
        let tx = MockTransport::new(
            status,
            vec![Chunk::Data(br#"{"error":{"message":"no"}}"#.to_vec())],
        );
        let o = go(ANT, REQ, &tx, &MemoryCredStore::new());
        assert_eq!(o.code, code, "status {status} → exit {code}");
    }
}

#[test]
fn a_malformed_2xx_body_is_502() {
    // A 2xx body that is not JSON is an upstream contract violation → Provider{502} → 70.
    let tx = MockTransport::ok(vec![b"not json"]);
    let o = go(ANT, REQ, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 70);
    assert!(o.stderr.contains("malformed token count"));
}

#[test]
fn a_2xx_body_missing_the_token_key_is_502() {
    // A well-formed 2xx with no `input_tokens` number is likewise a 502 (never a silent 0).
    let tx = MockTransport::ok(vec![br#"{"other":1}"#]);
    let o = go(ANT, REQ, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 70);
    assert!(o.stderr.contains("no `input_tokens` number"));
}

#[test]
fn a_mid_drain_drop_is_transport_69() {
    // A transport drop while draining the 2xx count body → Transport (69).
    let tx = MockTransport::new(
        200,
        vec![
            Chunk::Data(br#"{"input"#.to_vec()),
            Chunk::Fail(io::ErrorKind::UnexpectedEof),
        ],
    );
    let o = go(ANT, REQ, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 69);
    assert!(o.stderr.contains("failed to read token-count"));
}

#[test]
fn a_missing_input_file_is_no_input_66() {
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &[
            "--count-tokens",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--input",
            "/no/such/file",
        ],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 66);
    assert!(o.stderr.contains("cannot open --input file"));
}

#[test]
fn a_missing_attach_file_is_no_input_66() {
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &[
            "--count-tokens",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "-f",
            "/no/such/file",
        ],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 66);
    assert!(o.stderr.contains("cannot read --file"));
}

#[test]
fn a_malformed_stdin_request_is_usage_64() {
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(ANT, b"{ not json", &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 64);
}

#[test]
fn a_failing_request_read_is_usage_64() {
    // The read_request error path over a failing reader (the request never arrives).
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go_reader(ANT, &mut FailReader, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 64);
}

#[test]
fn a_stdout_write_failure_is_transport() {
    // The count decoded fine, but writing it fails → Transport (69).
    let tx = MockTransport::ok(vec![COUNT]);
    let mut reader = io::Cursor::new(REQ.to_vec());
    let mut out = FailWriter;
    let (code, stderr) = go_out(
        ANT,
        &[],
        &mut reader,
        &tx,
        &MemoryCredStore::new(),
        &mut out,
    );
    assert_eq!(code, 69);
    assert!(stderr.contains("failed to write token count"));
}

#[test]
fn an_empty_cache_with_no_model_is_config_78() {
    // A request with no model + an empty cache: `select_model` has nothing to default from
    // → Config (78), the same query error the generation path raises.
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        ANT,
        br#"{"messages":[{"role":"user","content":"hi"}]}"#,
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("anthropic"));
}
