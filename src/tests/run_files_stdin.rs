//! End-to-end `run` for the `-f -` stdin spelling (arch §5.5): `-` NAMES stdin, so
//! the piped bytes become the same `Content::Text` context part a named path yields
//! and `bz "q1" | bz -f - "q2"` chains two runs. Covers the two facts the named-path
//! module cannot: argv order across mixed sources, and that reading stdin here
//! EXHAUSTS it, so the canonical channel downstream sees EOF. `MockTransport`; zero
//! network. The `-f - -f -` and error-path rules are unit-tested in `pipeline_input`.

use crate::tests::run_support::*;

#[test]
fn dash_file_reads_stdin_as_a_context_part_before_the_prompt() {
    // `-f -` names stdin (§5.5): the piped bytes reach the wire as the same context
    // part a named file yields, still preceding the positional prompt. This is the
    // `bz "q1" | bz -f - "q2"` chain, end to end.
    let a = file_with(b"ctx-file");
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
            "-",
            "-f",
            a.path().to_str().unwrap(),
            "the-question",
        ],
        &[],
        b"piped-context",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    let piped = body.find("piped-context").expect("stdin on the wire");
    let file = body.find("ctx-file").expect("file on the wire");
    let q = body.find("the-question").expect("prompt on the wire");
    assert!(
        piped < file && file < q,
        "argv order holds across `-` and paths"
    );
}

#[test]
fn dash_file_consumes_stdin_so_the_canonical_channel_sees_eof() {
    // `-f -` EXHAUSTS stdin, so the no-prompt canonical arm reads `Ok(0)` and the
    // attachments alone are the message — the `-f`-plus-piped-request refusal cannot
    // fire on bytes the caller explicitly claimed as text (§5.5).
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
            "-",
        ],
        &[],
        br#"{"messages":[{"role":"user","content":"not parsed as a request"}]}"#,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0, "no 64 refusal: stdin was claimed by `-f -`");
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(
        body.contains("not parsed as a request"),
        "the JSON rode as literal text, not as a parsed request"
    );
}
