//! The `--raw` error paths (arch §5.4): the byte path has its OWN pre-flight error
//! handling (config/auth/transport) and stream-drop/broken-pipe branches, distinct from
//! the typed `generate` core — so they are exercised here directly rather than shared
//! with the canonical tests. The happy raw passthrough lives in `run_modes`.

use std::io;

use crate::testing::{Chunk, MockTransport};
use crate::tests::run_support::*;

/// A raw request with no resolvable provider fails at config resolution (78), in-band
/// through the sink before any bytes — `serve_raw`'s `into_resolved` error arm.
#[test]
fn raw_no_provider_is_config_error_78() {
    let o = go(&["--raw"], &[], b"{}", &ok_basic(), &empty_store());
    assert_eq!(o.code, 78);
}

/// A raw request to a keyed provider with no credential fails auth (77) — `serve_raw`'s
/// `auth.apply` error arm, before the request is sent.
#[test]
fn raw_missing_cred_is_auth_error_77() {
    let o = go(
        &["--raw", "--provider", "anthropic", "--model", "x"],
        &[],
        b"{}",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 77);
}

/// A transport handshake failure on the raw path is 69 — `serve_raw`'s `send` error arm.
#[test]
fn raw_transport_error_is_69() {
    let o = go(
        &[
            "--raw",
            "--provider",
            "anthropic",
            "--model",
            "x",
            "--api-key",
            "sk",
        ],
        &[],
        b"{}",
        &ErrTransport,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
}

/// A mid-stream transport drop on the raw path is 69: `stream_raw` ends the stream with
/// an in-band `Transport` error. The `RawSink` drops the error LINE, but the exit still
/// carries it (§5.4), so stdout stays empty and the code is 69.
#[test]
fn raw_transport_drop_is_69() {
    let tx = MockTransport::new(200, vec![Chunk::Fail(io::ErrorKind::ConnectionReset)]);
    let o = go(
        &[
            "--raw",
            "--provider",
            "anthropic",
            "--model",
            "x",
            "--api-key",
            "sk",
        ],
        &[],
        b"{}",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stdout.is_empty());
}

/// A broken pipe WHILE streaming the raw response maps to 141 (the Windows SIGPIPE path)
/// — `stream_raw`'s write-error arm: the `RawSink` write of the body bytes fails.
#[test]
fn raw_broken_pipe_during_stream_is_141() {
    let code = run_broken_pipe(
        &[
            "--raw",
            "--provider",
            "anthropic",
            "--model",
            "x",
            "--api-key",
            "sk",
        ],
        &empty_store(),
    );
    assert_eq!(code, 141);
}

/// A broken pipe WHILE reporting a pre-stream fatal maps to 141 — `fail_inband`'s
/// write-error arm. Empty stdin under `--json` is a parse error (64), reported in-band
/// as an NDJSON line that the broken stdout rejects, so the exit becomes 141 instead.
#[test]
fn preflight_error_broken_pipe_is_141() {
    let code = run_broken_pipe(&["--json"], &empty_store());
    assert_eq!(code, 141);
}
