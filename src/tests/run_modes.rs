//! End-to-end `run` (arch §9.6) — the output projections: text/json/raw/thinking.
//! The input channels (positional/stdin/`--input`) live in `run_inputs`. Driven by
//! `MockTransport`; zero network.

use crate::testing::MockTransport;
use crate::tests::run_support::*;

#[test]
fn text_default_concatenates_text_no_end_line() {
    // A prefix-owned `--model` so no model-list probe fires — these mode tests assert
    // the output projection over one generation round-trip, not model discovery.
    let o = go(
        &[
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
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    assert!(o.stderr.is_empty());
}

#[test]
fn text_on_a_tty_is_pretty_answer_pristine_chrome_on_stderr() {
    // The interactive skin (interactive-output §3–§5): `stdout_tty` + a real TERM/locale
    // resolve to `PrettySink`. The answer on stdout stays byte-identical to the plain
    // text mode (no SGR), while the finish/usage footer lands on stderr.
    let o = go_pretty(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello"); // pristine — the building-block contract
    assert_eq!(
        o.stderr,
        "\u{1b}[32m✓\u{1b}[0m \u{1b}[2mstop · 12 in · 2 out\u{1b}[0m\n"
    );
}

#[test]
fn json_emits_the_event_stream_ending_in_end() {
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
            "--thinking",
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
    // --raw skips encode, but the wire still targets `{base_url}{path}` — not the
    // empty url that made every raw request a connect error (bl-080b). MockTransport
    // ignores the url, so this assertion is the offline guard the bug slipped past.
    let sent = tx.requests();
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
    assert_eq!(sent[0].body, b"REQUEST");
    // --raw skips encode, but the wire STILL carries the dialect's content-type —
    // `serve` stamps `Protocol::content_type()` for both paths. Without it a
    // JSON-body provider can't parse the verbatim body (bl-da81: openai
    // chat/completions 400s a content-type-less POST). This is the offline guard.
    assert_eq!(sent[0].header("content-type"), Some("application/json"));
    // --raw also inherits the row's static beta_headers — without anthropic-version every Anthropic raw request 400s (bl-3e2f); serve stamps ctx.beta_headers for both paths, the guard this test earlier lacked.
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
}
