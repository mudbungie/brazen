//! End-to-end `run` (arch §9.6) — resolved config values reaching the wire: the
//! stored credential on the auth header, the implicit `stream:true` default, and
//! the stamped transport timeouts (floor and flag override). The error paths live
//! in `run_config`. Driven by `MockTransport`; zero network.

use crate::testing::MemoryCredStore;
use crate::tests::run_support::*;
use crate::{Cred, Secret, Timeouts};

#[test]
fn credential_from_store_is_used() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-store"),
        },
    );
    let tx = ok_basic();
    let o = go(
        &["--provider", "anthropic", "--model", "claude-x", "hi"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn streaming_is_the_default_on_the_wire() {
    // No --stream and no config stream: serve requests streaming implicitly so the
    // SSE decoder in `drive` has a stream to frame (bl-20d5). The bare `bz <prompt>`
    // path now works against a framed provider without an explicit flag.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("k"),
        },
    );
    let tx = ok_basic();
    // `--model claude-x` is prefix-owned, so no model-list probe fires (this asserts
    // the implicit-stream default, not model discovery): one round-trip.
    let o = go(
        &["--provider", "anthropic", "--model", "claude-x", "hi"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(body.contains(r#""stream":true"#), "default body: {body}");
}

#[test]
fn run_stamps_the_resolved_timeouts_on_the_wire() {
    // `serve` stamps the resolved transport bounds onto the request the transport
    // consumes — the embedded `defaults.toml` floor unless a flag overrides it.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk"),
        },
    );
    let tx = ok_basic();
    // A prefix-owned `--model` so no probe fires — this asserts the stamped timeouts on
    // the one generation request.
    let o = go(
        &["--provider", "anthropic", "--model", "claude-x", "hi"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(
        tx.requests()[0].timeouts,
        Timeouts {
            connect: Some(30),
            response: Some(120),
            idle: Some(300),
        }
    );

    // A flag overrides the floor; the override reaches the wire.
    let tx2 = ok_basic();
    let o2 = go(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--timeout-idle",
            "7",
            "hi",
        ],
        &[],
        b"",
        &tx2,
        &store,
    );
    assert_eq!(o2.code, 0);
    assert_eq!(tx2.requests()[0].timeouts.idle, Some(7));
}
