//! End-to-end `run` (arch §9.6) — output projections and the two input channels:
//! text/json/raw/thinking, positional-vs-stdin parity, `--input FILE`, and the
//! input-error exit codes (64/66). Driven by `MockTransport`; zero network.

mod run_support;

use brazen::testing::MockTransport;
use run_support::*;

// ============================ projections ============================

#[test]
fn text_default_concatenates_text_no_end_line() {
    let o = go(
        &["hi", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    assert!(o.stderr.is_empty());
}

#[test]
fn json_emits_the_event_stream_ending_in_end() {
    let o = go(
        &["hi", "--json", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains(r#""type":"message_start""#));
    assert!(o.stdout.contains(r#""text_delta":"Hel""#));
    assert!(o.stdout.trim_end().ends_with(r#"{"type":"end"}"#));
}

#[test]
fn thinking_flag_plumbs_through_text_mode() {
    let o = go(
        &[
            "hi",
            "--thinking",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
}

#[test]
fn raw_passes_provider_bytes_through_verbatim() {
    let tx = MockTransport::ok(vec![b"server-native-bytes"]);
    let o = go(
        &["--raw", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"REQUEST",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "server-native-bytes");
}

// ============================ input channels ============================

#[test]
fn positional_and_stdin_build_the_same_wire_request() {
    let tx_pos = ok_basic();
    let _ = go(
        &["hi", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"",
        &tx_pos,
        &empty_store(),
    );
    let body_pos = tx_pos.requests()[0].body.clone();

    let stdin = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
    let tx_stdin = ok_basic();
    let _ = go(
        &["--provider", "anthropic", "--api-key", "sk"],
        &[],
        stdin,
        &tx_stdin,
        &empty_store(),
    );
    let body_stdin = tx_stdin.requests()[0].body.clone();

    assert_eq!(body_pos, body_stdin);
}

#[test]
fn stdin_request_model_routes_and_reaches_the_wire() {
    let stdin = br#"{"model":"claude-x","messages":[{"role":"user","content":"hi"}]}"#;
    let tx = ok_basic();
    let o = go(
        &["--provider", "anthropic", "--api-key", "sk", "--json"],
        &[],
        stdin,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(body.contains("claude-x"));
}

#[test]
fn positional_prompt_wins_and_ignores_piped_stdin() {
    // POSIX filter idiom (§5.5): the positional is the explicit signal; piped
    // stdin is simply not consumed (the writer's concern via SIGPIPE), so this
    // succeeds with the prompt rather than erroring on "two inputs".
    let tx = ok_basic();
    let o = go(
        &["hi", "--text", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        br#"{"model":"from-stdin","messages":[{"role":"user","content":"ignored"}]}"#,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    // The wire request was built from the prompt, never the ignored stdin.
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(body.contains("hi"));
    assert!(!body.contains("from-stdin"));
}

#[test]
fn malformed_stdin_json_is_parse_input_64() {
    let o = go(
        &["--json", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"not json",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stdout.contains(r#""type":"error""#));
}

#[test]
fn input_file_and_stdin_are_parity() {
    let bytes = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
    let f = temp(std::str::from_utf8(bytes).unwrap());
    let path = f.0.to_str().unwrap();

    let tx_file = ok_basic();
    let _ = go(
        &[
            "--input",
            path,
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx_file,
        &empty_store(),
    );

    let tx_stdin = ok_basic();
    let _ = go(
        &["--provider", "anthropic", "--api-key", "sk"],
        &[],
        bytes,
        &tx_stdin,
        &empty_store(),
    );

    assert_eq!(tx_file.requests()[0].body, tx_stdin.requests()[0].body);
}

#[test]
fn missing_input_file_is_noinput_66() {
    let missing = std::env::temp_dir().join("brazen_absent_run.json");
    let o = go(
        &[
            "--input",
            missing.to_str().unwrap(),
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 66);
    assert!(o.stderr.contains("cannot open --input"));
}

#[test]
fn raw_body_read_error_is_64() {
    let o = go_reader(
        &["--raw", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        &mut FailReader,
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
}
