//! End-to-end `run` honoring the `stream` tri-state (config §4.2): an explicit
//! `stream:false` reaches the wire and `drive` folds the resulting single-JSON 2xx
//! body whole via `decode_full` (never the framed stream); `--no-stream` is the flag
//! form of the same intent. Driven by `MockTransport`; zero network. Split from
//! `run_config` to keep both under the repo's 300-line code-file cap.

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::run_support::*;
use crate::{Cred, Secret};

/// A COMPLETE non-stream Anthropic Messages 2xx body (config §4.2): the aggregate
/// `stream:false` returns, folded whole by `decode_full`. One text block + a finish.
const NONSTREAM_MSG: &[u8] = br#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-m","content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":1}}"#;

#[test]
fn an_explicit_stream_false_request_reaches_the_wire() {
    // brazen HONORS the stream tri-state, never silently reverts it (config §4.2): an
    // explicit `stream:false` reaches the wire as-is, and `drive` folds the resulting
    // single-JSON 2xx body whole via the protocol's `decode_full` (no framing). The
    // body here is a complete non-stream Messages response, so the run decodes a clean
    // canonical stream and exits 0 — the inverse of the removed always-stream force.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("k"),
        },
    );
    let tx = MockTransport::ok(vec![NONSTREAM_MSG]);
    // A prefix-owned model so the request is one round-trip (no probe) — this asserts
    // the honored `stream:false`, not model discovery.
    let req = br#"{"model":"claude-m","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let o = go(
        &["--provider", "anthropic", "--json"],
        &[],
        req,
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(
        body.contains(r#""stream":false"#),
        "honored non-stream body: {body}"
    );
    // The whole-body fold decoded the aggregate to a canonical stream (text + finish).
    assert!(
        o.stdout.contains(r#""text_delta":"Hi""#),
        "decoded: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains(r#""type":"finish""#),
        "decoded: {}",
        o.stdout
    );
}

#[test]
fn no_stream_flag_honors_non_stream_and_folds_the_whole_body() {
    // `--no-stream` (config §4.2) sets the tri-state to `false` with no request key;
    // `fill_absent` folds it onto the wire and `serve` honors it. A non-stream 2xx
    // body folds whole via `decode_full` to a clean canonical stream, exit 0.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("k"),
        },
    );
    let tx = MockTransport::ok(vec![NONSTREAM_MSG]);
    let o = go(
        &[
            "hi",
            "--no-stream",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
        ],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(body.contains(r#""stream":false"#), "no-stream body: {body}");
    assert!(
        o.stdout.contains(r#""type":"finish""#),
        "decoded: {}",
        o.stdout
    );
}
