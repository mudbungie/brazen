//! End-to-end `run` (arch §9.6) — the two terminal-event guarantees on FAILURE paths
//! (bl-7847, arch §5.6 / §3.2): (1) an open content block is closed with a synthesized
//! `ContentStop` before the injected `Error` on a premature EOF or a mid-stream drop, so
//! 'every ContentStart is eventually stopped' holds on failure exactly as on a clean
//! stream; (2) a non-stream `decode_full` yielding NEITHER a `Finish` nor an `Error` is a
//! malformed aggregate → in-band `Error{Transport}`/69, never a silently-empty exit-0
//! turn. Both additive (events grow on failure paths, none removed). Driven by
//! `MockTransport`; zero network.

use std::io;

use crate::testing::{Chunk, MockTransport};
use crate::tests::run_support::*;

/// The drain-on-error contract (§5.6): a premature EOF while a content block is still
/// OPEN synthesizes that block's `ContentStop` BEFORE the injected `Error`, so 'every
/// ContentStart is eventually stopped' holds on failure exactly as on a clean stream —
/// a consumer finalizing per-block state on `ContentStop` never leaks the open block.
#[test]
fn premature_eof_closes_open_block_before_error() {
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
        &MockTransport::ok(vec![OPEN_BLOCK]),
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    let stop = o
        .stdout
        .find(r#""type":"content_stop""#)
        .expect("ContentStop synthesized for the open block");
    let err = o
        .stdout
        .find(r#""type":"error""#)
        .expect("injected Error present");
    assert!(stop < err, "ContentStop must precede the injected Error");
    assert!(o.stdout.contains("premature upstream EOF"));
    assert!(o.stdout.trim_end().ends_with(r#"{"type":"end"}"#));
}

/// The transport-drop sibling of the drain contract (§5.6): a mid-stream drop while a
/// block is OPEN also closes it with a synthesized `ContentStop` before the `Error`.
#[test]
fn transport_drop_closes_open_block_before_error() {
    let tx = MockTransport::new(
        200,
        vec![
            Chunk::Data(OPEN_BLOCK.to_vec()),
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
    let stop = o
        .stdout
        .find(r#""type":"content_stop""#)
        .expect("ContentStop synthesized for the open block");
    let err = o
        .stdout
        .find(r#""type":"error""#)
        .expect("injected Error present");
    assert!(stop < err, "ContentStop must precede the injected Error");
    assert!(o.stdout.contains("transport stream dropped"));
}

/// The non-stream completeness guard (§3.2), one fixture PER structureless/structured
/// dialect: an empty/finish-less 200 aggregate (`{}`, `{"choices":[]}`) folds through
/// `choices[0]`-Null tolerance to a bare `MessageStart` — NO Finish, NO Error — which
/// used to exit 0 as a silently-empty successful turn. Now `run` appends an in-band
/// `Transport` error so the empty turn surfaces (exit 69, "no completion"), never a
/// silent success. Transport (not ParseInput/64): the request earned a 200, so the
/// fault is the RESPONSE — the mirror of the streaming fold's premature-EOF error.
#[test]
fn empty_nonstream_aggregate_surfaces_a_completion_error_per_dialect() {
    const ANTHROPIC: &[u8] =
        include_bytes!("../../tests/fixtures/anthropic_messages_nonstream_empty.json");
    const OPENAI: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_nonstream_empty.json");
    const GOOGLE: &[u8] = include_bytes!("../../tests/fixtures/google_genai_nonstream_empty.json");
    const OLLAMA: &[u8] = include_bytes!("../../tests/fixtures/ollama_chat_nonstream_empty.json");
    let req = br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    for (provider, body) in [
        ("anthropic", ANTHROPIC),
        ("openai", OPENAI),
        ("google", GOOGLE),
        ("ollama", OLLAMA),
    ] {
        // ollama's row is auth=none — passing an api-key would be inert, so omit it.
        let mut argv = vec!["--json", "--provider", provider];
        if provider != "ollama" {
            argv.extend(["--api-key", "sk"]);
        }
        let o = go(
            &argv,
            &[],
            req,
            &MockTransport::ok(vec![body]),
            &empty_store(),
        );
        assert_eq!(
            o.code, 69,
            "{provider}: a verdict-less aggregate must be 69"
        );
        assert!(
            o.stdout
                .contains("non-stream response carried no completion"),
            "{provider}: the completeness error must surface"
        );
        assert!(
            o.stdout.trim_end().ends_with(r#"{"type":"end"}"#),
            "{provider}: still terminated by the one End"
        );
    }
}

/// The fifth `decode_full` path — `openai-responses` — is the guard's NO-OP witness: it
/// has a native in-body terminator, so even an empty `{}` response synthesizes
/// `response.completed` → `Finish{Stop}`. Its empty aggregate is therefore a DEGENERATE
/// SUCCESS (exit 0, a Finish, no Error), NOT a malformed one — the guard fires only on a
/// genuinely verdict-less body, never on a dialect that self-terminates.
#[test]
fn empty_responses_aggregate_self_terminates_exit_0() {
    const RESPONSES: &[u8] =
        include_bytes!("../../tests/fixtures/openai_responses_nonstream_empty.json");
    let req = br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let o = go(
        &[
            "--json",
            "--provider",
            "openai-responses",
            "--api-key",
            "sk",
        ],
        &[],
        req,
        &MockTransport::ok(vec![RESPONSES]),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains(r#""type":"finish""#));
    assert!(!o.stdout.contains(r#""type":"error""#));
}
