//! `bz --in DIALECT` end to end (ingress.md §11, §14): the one-shot masquerade
//! filter — a dialect request on stdin, the dialect response on stdout, both
//! shapes (§10); the flag-conflict 64s; the §9 error masquerade in-band; and
//! the `--raw=out` composition. Driven through `run` with the stub upstream
//! `MockTransport` — the end-to-end masquerade test: dialect in, canonical
//! pipeline, dialect out.

use std::io::Cursor;

use crate::tests::run_support::*;

/// A minimal config routing the masquerade model name to the anthropic row.
fn masq_cfg() -> TempFile {
    temp(
        r#"
api_key = "sk-test"

[[provider]]
name = "anthropic"
model_aliases = { "gpt-4o" = "claude-x" }

# The shipped openai row owns gpt-* by prefix; clear it so the alias is the
# ONE owner (the masquerade recipe's disambiguation line).
[[provider]]
name = "openai"
model_prefixes = []
"#,
    )
}

fn go_in(stdin: &[u8], tx: &dyn crate::Transport) -> (Out, TempFile) {
    let cfg = masq_cfg();
    let out = go(
        &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
        &[],
        stdin,
        tx,
        &empty_store(),
    );
    (out, cfg)
}

#[test]
fn an_aggregate_request_folds_one_chat_completion_body() {
    let (o, _cfg) = go_in(
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
        &ok_basic(),
    );
    assert_eq!(o.code, 0, "stdout: {} stderr: {}", o.stdout, o.stderr);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hello");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["model"], "claude-opus-4-8", "upstream identity wins");
}

#[test]
fn stream_true_selects_sse_frames_on_stdout() {
    let (o, _cfg) = go_in(
        br#"{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        &ok_basic(),
    );
    assert_eq!(o.code, 0);
    assert!(
        o.stdout.contains(r#""object":"chat.completion.chunk""#),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains(r#"{"content":"Hel"}"#));
    assert!(o.stdout.ends_with("data: [DONE]\n\n"), "{}", o.stdout);
}

#[test]
fn an_upstream_error_masquerades_with_the_carried_status() {
    // Upstream 500: the §9 envelope on stdout, the run's own exit 70 (§8).
    let tx = crate::testing::MockTransport::new(
        500,
        vec![crate::testing::Chunk::Data(
            br#"{"type":"error","error":{"type":"api_error","message":"upstream broke"}}"#.to_vec(),
        )],
    );
    let (o, _cfg) = go_in(
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
        &tx,
    );
    assert_eq!(o.code, 70);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["error"]["code"], 500);
    assert_eq!(body["error"]["type"], "server_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("upstream broke"));
}

#[test]
fn a_non_json_stdin_is_the_dialect_400_envelope_and_exit_64() {
    let (o, _cfg) = go_in(b"not json at all", &ok_basic());
    assert_eq!(o.code, 64);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["error"]["code"], 400);
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[test]
fn a_config_failure_after_decode_is_the_dialect_500_envelope_and_exit_78() {
    // Config resolution fails AFTER decode (a row whose empty `model_prefixes`
    // element would own every model is `BadValue`/78, config §7) — in-band per §9,
    // never stderr. Two rows OWNING one model is no longer a failure at all: the
    // priority list picks the first (arch §4.3, `config_priority`).
    let cfg = temp(
        r#"
api_key = "sk"

[[provider]]
name = "anthropic"
model_prefixes = [""]
"#,
    );
    let o = go(
        &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
        &[],
        br#"{"model":"gpt-4o","messages":[]}"#,
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78, "stdout: {} stderr: {}", o.stdout, o.stderr);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["error"]["code"], 500);
    assert!(o.stderr.is_empty(), "post-decode failures are in-band");
}

#[test]
fn raw_out_composes_streaming_the_provider_bytes_verbatim() {
    // `--in x --raw=out` (§11): dialect request in, the provider's EXACT bytes out.
    let cfg = masq_cfg();
    let tx = ok_basic();
    let o = go(
        &[
            "--in",
            "openai_chat",
            "--raw=out",
            "--config",
            cfg.0.to_str().unwrap(),
        ],
        &[],
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout.as_bytes(), BASIC, "verbatim upstream bytes");
    // And the request half really was the DIALECT decode: the wire body is the
    // encoded anthropic request for the alias-substituted model.
    let sent = tx.requests();
    assert!(String::from_utf8_lossy(&sent[0].body).contains(r#""model":"claude-x""#));
}

#[test]
fn raw_out_surfaces_a_decode_failure_as_the_exit_alone() {
    let cfg = masq_cfg();
    let o = go(
        &[
            "--in",
            "openai_chat",
            "--raw=out",
            "--config",
            cfg.0.to_str().unwrap(),
        ],
        &[],
        b"not json",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(
        o.code, 64,
        "the RawSink drops the line; the exit carries it"
    );
    assert!(o.stdout.is_empty());
}

#[test]
fn a_failing_stdin_read_is_in_band_on_both_compositions() {
    let cfg = masq_cfg();
    let argv = ["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()];
    let o = go_reader(&argv, &[], &mut FailReader, &ok_basic(), &empty_store());
    assert_eq!(o.code, 64);
    assert!(
        o.stdout.contains(r#""type":"invalid_request_error""#),
        "{}",
        o.stdout
    );

    let argv = [
        "--in",
        "openai_chat",
        "--raw=out",
        "--config",
        cfg.0.to_str().unwrap(),
    ];
    let o = go_reader(&argv, &[], &mut FailReader, &ok_basic(), &empty_store());
    assert_eq!(o.code, 64);
    assert!(o.stdout.is_empty());
}

#[test]
fn a_dead_stdout_maps_through_from_io() {
    // The StdoutRespond write fails on the first chunk → the SIGPIPE class (141).
    let cfg = masq_cfg();
    let mut out = BrokenPipeWriter;
    let mut err = Vec::new();
    let clock = crate::testing::FakeClock::new(0);
    let cache = crate::testing::MemoryModelCache::new();
    let stash = unused_stash();
    let tx = ok_basic();
    let store = empty_store();
    let host = host(&tx, &store, &cache, &clock, &stash);
    let code = crate::run(
        args(
            &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
            &[],
        ),
        &mut Cursor::new(br#"{"model":"gpt-4o","messages":[]}"#.to_vec()),
        &mut out,
        &mut err,
        &host,
    );
    assert_eq!(code, 141);
}

// ---- the §11 conflict 64s ----

#[test]
fn in_conflicts_with_a_positional_prompt() {
    let o = go(
        &["--in", "openai_chat", "hi there"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stderr.contains("positional prompt"), "{}", o.stderr);
}

#[test]
fn in_conflicts_with_raw_in_including_the_derived_spelling() {
    for raw in ["--raw=in", "--raw", "--raw=both"] {
        let o = go(
            &["--in", "openai_chat", raw],
            &[],
            b"{}",
            &ok_basic(),
            &empty_store(),
        );
        assert_eq!(o.code, 64, "{raw}");
        assert!(o.stderr.contains("--raw=in"), "{raw}: {}", o.stderr);
    }
}

#[test]
fn in_conflicts_with_file_attachments() {
    let o = go(
        &["--in", "openai_chat", "-f", "somefile.txt"],
        &[],
        b"{}",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stderr.contains("--file cannot be combined with --in"));
}

#[test]
fn an_unknown_dialect_name_is_usage_64() {
    let o = go(
        &["--in", "responses_chat"],
        &[],
        b"{}",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(
        o.stderr.contains("openai_chat") && o.stderr.contains("anthropic_messages"),
        "names the known set: {}",
        o.stderr
    );
}
