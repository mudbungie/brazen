//! End-to-end `run` for `-f`/`--file` content-attach (arch §5.5, §9.6): files
//! become `Content::Text` parts preceding the positional prompt in the one user
//! message, the file errors map to exit 66 (like `--input`), and `-f` refuses both
//! the verbatim `--raw` path and a piped canonical request. Driven by
//! `MockTransport`; zero network.

use tempfile::NamedTempFile;

use crate::testing::MockTransport;
use crate::tests::run_support::*;

/// A temp file holding `bytes` (arbitrary, incl. non-UTF-8): a unique `tempfile`,
/// auto-removed on drop — keep the handle alive for the duration of the `run`.
fn file_with(bytes: &[u8]) -> NamedTempFile {
    let f = NamedTempFile::new().unwrap();
    std::fs::write(f.path(), bytes).unwrap();
    f
}

#[test]
fn prompt_with_files_composes_files_then_prompt_on_the_wire() {
    // The user message is `[file₁, file₂, prompt]` in argv order (§5.5) — proven by the
    // three texts reaching the encoded body with their positions strictly increasing.
    let a = file_with(b"ctx-one");
    let b = file_with(b"ctx-two");
    let tx = ok_basic();
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "-f",
            a.path().to_str().unwrap(),
            "--file",
            b.path().to_str().unwrap(),
            // The positional prompt is LAST (options-before-prompt, §5.5).
            "the-question",
        ],
        &[],
        b"", // stdin unread — the positional wins
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    let one = body.find("ctx-one").expect("file one on the wire");
    let two = body.find("ctx-two").expect("file two on the wire");
    let q = body.find("the-question").expect("prompt on the wire");
    assert!(one < two && two < q, "files precede the prompt, in order");
}

#[test]
fn files_only_no_prompt_builds_a_user_message_from_the_files() {
    // Bare `-f` (no prompt, empty stdin) → one user message of just the file parts (§5.5).
    let a = file_with(b"sole-context");
    let tx = ok_basic();
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "--file",
            a.path().to_str().unwrap(),
        ],
        &[],
        b"", // empty stdin == "no request" → files-only, not a refusal
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(body.contains("sole-context"));
}

#[test]
fn missing_file_is_noinput_66() {
    // A missing `-f` file is exit 66 on stderr (the pre-sink fatal, like `--input`).
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.txt");
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--file",
            missing.to_str().unwrap(),
            "hi",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 66);
    assert!(o.stderr.contains("cannot read --file"));
}

#[test]
fn non_utf8_file_is_noinput_66() {
    // A text part is UTF-8, so a non-UTF-8 file fails the same way a missing one does (66).
    let bin = file_with(&[0xff, 0xfe, 0x00, 0x80]);
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "-f",
            bin.path().to_str().unwrap(),
            "hi",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 66);
    assert!(o.stderr.contains("cannot read --file"));
}

#[test]
fn files_plus_a_piped_canonical_request_is_refused_64() {
    // No prompt + `-f` + a real request on stdin has no single merge → refuse (§5.5).
    // Text mode routes the in-band error to stderr.
    let a = file_with(b"ctx");
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--file",
            a.path().to_str().unwrap(),
        ],
        &[],
        br#"{"messages":[{"role":"user","content":"hi"}]}"#,
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stderr.contains("cannot combine --file"));
}

#[test]
fn file_with_raw_is_refused_64() {
    // `--raw` sends the body verbatim and runs no constructor → `-f` is incompatible (§5.5).
    let a = file_with(b"ctx");
    let o = go(
        &[
            "--raw",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "-f",
            a.path().to_str().unwrap(),
        ],
        &[],
        b"REQUEST",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stderr.contains("--file cannot be combined with --raw"));
}

#[test]
fn file_with_raw_in_is_refused_but_composes_with_raw_out() {
    // The `-f` refusal keys on the INPUT axis (§5.4/§13.14): `--raw=in` runs no constructor,
    // so `-f` is refused (64) exactly as bare `--raw`.
    let a = file_with(b"ctx");
    let refused = go(
        &[
            "--raw=in",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "-f",
            a.path().to_str().unwrap(),
        ],
        &[],
        b"REQUEST",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(refused.code, 64);
    assert!(refused
        .stderr
        .contains("--file cannot be combined with --raw"));
    // `--raw=out` DOES run the constructor, so `-f` composes: the file text is encoded into
    // the request body, and the response streams back verbatim.
    let b = file_with(b"attached-ctx");
    let tx = MockTransport::ok(vec![b"WIRE"]);
    let composed = go(
        &[
            "--raw=out",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "-f",
            b.path().to_str().unwrap(),
            "q",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(composed.code, 0);
    assert_eq!(composed.stdout, "WIRE");
    assert!(String::from_utf8_lossy(&tx.requests()[0].body).contains("attached-ctx"));
}

#[test]
fn file_attaches_on_a_bare_tty_without_the_usage_hint() {
    // `-f` is itself a request source, so a tty invocation with files does NOT print the
    // bare-invocation usage hint — it attaches and runs (the shim injects empty stdin).
    let a = file_with(b"tty-context");
    let o = go_tty(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "--file",
            a.path().to_str().unwrap(),
        ],
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
}
