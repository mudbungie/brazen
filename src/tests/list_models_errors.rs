//! End-to-end `bz --list-models` error paths (model-discovery §2): unknown
//! provider (78), missing credential (77), the non-2xx status→exit mapping with
//! the body carried, a malformed body (70), a mid-body drop (69), a failing
//! stdout (69), and a verb argv usage error (64). The happy paths live in
//! `list_models`; the shared harness in `list_models_support`. Offline.

use std::collections::BTreeMap;
use std::io;

use crate::testing::{Chunk, MemoryCredStore, MockTransport};
use crate::tests::list_models_support::{go, go_out, FailWriter, ANT, MODELS};
use crate::EnvSnapshot;

#[test]
fn unknown_provider_is_config_78() {
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["--list-models", "--provider", "nope", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 78);
    assert!(tx.requests().is_empty());
}

#[test]
fn missing_credential_is_auth_77() {
    // No `--api-key` and an empty store → `Auth::apply` fails MissingCreds → 77.
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["--list-models", "--provider", "anthropic"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 77);
    assert!(tx.requests().is_empty()); // auth failed before the send
}

#[test]
fn a_non_2xx_models_response_maps_the_status_and_carries_the_body() {
    // The discovery path drains the non-2xx body and routes it through the SAME
    // `http_error` home the data plane uses (bl-dcfe): the status drives the exit AND
    // the body reaches the user as `message` (the verb's stderr channel), never a
    // bespoke "HTTP {status}" that throws it away. Each case is a distinct envelope —
    // `error.message`, a `detail` (codex shape), a bare plain body — so the projection
    // assumes no uniform `{"error":…}` shape; the `want` substring is the lifted message.
    for (status, body, exit, want) in [
        (
            503u16,
            &br#"{"error":{"message":"is down"}}"#[..],
            70,
            "is down",
        ),
        (401, &br#"{"detail":"bad version"}"#[..], 77, "bad version"),
        (404, &b"no route"[..], 69, "no route"),
    ] {
        let tx = MockTransport::new(status, vec![Chunk::Data(body.to_vec())]);
        let o = go(
            &[
                "--list-models",
                "--provider",
                "anthropic",
                "--api-key",
                "sk",
            ],
            &tx,
            &MemoryCredStore::new(),
        );
        assert_eq!(o.code, exit, "{status} → exit {exit}");
        assert!(
            o.stderr.contains(want),
            "{status} carries body: {}",
            o.stderr
        );
    }
}

#[test]
fn a_malformed_body_is_provider_70() {
    // A drained 2xx that does not project to the list shape is the `Provider{502}`
    // `decode_models` raises → exit 70 (model-discovery §2).
    let tx = MockTransport::ok(vec![b"{not json"]);
    let o = go(ANT, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 70);
    assert!(o.stderr.contains("malformed models list"));
}

#[test]
fn a_mid_body_transport_drop_is_69() {
    // A 200 whose body fails part-way (an injected mid-stream drop) → `drain` surfaces
    // a `Transport` error → 69.
    let tx = MockTransport::new(
        200,
        vec![
            Chunk::Data(br#"{"data":["#.to_vec()),
            Chunk::Fail(io::ErrorKind::ConnectionReset),
        ],
    );
    let o = go(ANT, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 69);
    assert!(o.stderr.contains("failed to read models response body"));
}

#[test]
fn a_stdout_write_failure_is_69() {
    // The listing cannot be written (a closed/failing stdout) → `Transport` (→69), the
    // verb's pre-sink analogue of the data plane's write handling.
    let tx = MockTransport::ok(vec![MODELS]);
    let (code, stderr) = go_out(
        ANT,
        &EnvSnapshot(BTreeMap::new()),
        &tx,
        &MemoryCredStore::new(),
        &mut FailWriter,
    );
    assert_eq!(code, 69);
    assert!(stderr.contains("failed to write model list"));
}

#[test]
fn a_usage_error_in_the_verb_argv_is_64() {
    // The verb reuses the full flag parser → an unknown flag is the same usage error 64.
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["--list-models", "--provider", "anthropic", "--bogus"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 64);
    assert!(tx.requests().is_empty());
}
