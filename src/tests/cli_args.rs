//! Flag parsing (arch §5.5, §5.9): every recognized flag sets the flag-layer
//! config or a pre-resolve field; usage errors (exit 64) are surfaced, never
//! silently absorbed. Pure over a `&[String]`, so no process argv is touched.

use crate::{parse_args, Content, OutMode};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

#[test]
fn positional_prompt_is_captured() {
    let f = parse_args(&argv(&["what is 2+2"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("what is 2+2"));
    assert!(f.config.output.is_none());
}

#[test]
fn output_mode_flags() {
    assert_eq!(
        parse_args(&argv(&["--text"])).unwrap().config.output,
        Some(OutMode::Text)
    );
    assert_eq!(
        parse_args(&argv(&["--json"])).unwrap().config.output,
        Some(OutMode::Ndjson)
    );
    assert_eq!(
        parse_args(&argv(&["--raw"])).unwrap().config.output,
        Some(OutMode::Raw)
    );
}

#[test]
fn boolean_flags_thinking_stream_dump() {
    let f = parse_args(&argv(&["--thinking", "--stream", "--dump-config"])).unwrap();
    assert_eq!(f.config.thinking, Some(true));
    assert_eq!(f.config.stream, Some(true));
    assert!(f.dump_config);
}

#[test]
fn help_and_version_flags_with_aliases() {
    // The two discovery short-circuits set their pre-resolve bit; `run` acts on it.
    for spelling in ["--help", "-h"] {
        let f = parse_args(&argv(&[spelling])).unwrap();
        assert!(f.help, "{spelling} should set help");
        assert!(!f.version);
    }
    for spelling in ["--version", "-V"] {
        let f = parse_args(&argv(&[spelling])).unwrap();
        assert!(f.version, "{spelling} should set version");
        assert!(!f.help);
    }
}

#[test]
fn no_stream_sets_the_tri_state_false() {
    // `--no-stream` is the explicit non-stream intent (config §4.2), honored on the
    // wire via `decode_full` — the `--stream` sibling that sets `Some(false)`.
    let f = parse_args(&argv(&["--no-stream"])).unwrap();
    assert_eq!(f.config.stream, Some(false));
}

#[test]
fn value_flags_space_form() {
    let f = parse_args(&argv(&[
        "--provider",
        "anthropic",
        "--model",
        "claude",
        "--api-key",
        "sk-1",
        "--max-tokens",
        "256",
        "--temperature",
        "0.5",
        "--top-p",
        "0.9",
        "--input",
        "/tmp/req.json",
        "--config",
        "/tmp/cfg.toml",
    ]))
    .unwrap();
    assert_eq!(f.config.provider.as_deref(), Some("anthropic"));
    assert_eq!(f.config.model.as_deref(), Some("claude"));
    assert_eq!(
        f.config.api_key.map(|s| s.expose().to_owned()),
        Some("sk-1".to_owned())
    );
    assert_eq!(f.config.max_tokens, Some(256));
    assert_eq!(f.config.temperature, Some(0.5));
    assert_eq!(f.config.top_p, Some(0.9));
    assert_eq!(f.input.unwrap().to_str(), Some("/tmp/req.json"));
    assert_eq!(f.config_path.unwrap().to_str(), Some("/tmp/cfg.toml"));
}

#[test]
fn system_flag_is_one_text_content() {
    // The single-string flag form decodes to one `Content::Text`, the same shape
    // a bare file-array string yields — flags and file are one schema (config §2).
    let f = parse_args(&argv(&["--system", "be terse"])).unwrap();
    assert_eq!(
        f.config.system,
        Some(vec![Content::Text("be terse".into())])
    );
    // Equals form too.
    let g = parse_args(&argv(&["--system=be brief"])).unwrap();
    assert_eq!(
        g.config.system,
        Some(vec![Content::Text("be brief".into())])
    );
}

#[test]
fn file_flag_is_repeatable_and_accumulates_in_argv_order() {
    // `-f`/`--file` accumulates (NOT last-wins, unlike `--input`): both spellings and
    // the `=` form push, in order (§5.5 content-attach).
    let f = parse_args(&argv(&[
        "-f",
        "a.txt",
        "--file",
        "b.txt",
        "--file=c.txt",
        "--input",
        "/tmp/req.json",
    ]))
    .unwrap();
    let got: Vec<&str> = f.files.iter().map(|p| p.to_str().unwrap()).collect();
    assert_eq!(got, ["a.txt", "b.txt", "c.txt"]);
    // `--input` stays its own single last-wins slot, untouched by `-f`.
    assert_eq!(f.input.unwrap().to_str(), Some("/tmp/req.json"));
}

#[test]
fn no_file_flag_leaves_files_empty() {
    // The general path with empty inputs: a run with no `-f` simply has no attachments.
    assert!(parse_args(&argv(&["hi"])).unwrap().files.is_empty());
}

#[test]
fn file_flag_missing_value_is_usage_64() {
    let err = parse_args(&argv(&["--file"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("needs a value"));
}

#[test]
fn value_flags_equals_form() {
    let f = parse_args(&argv(&["--model=gpt-4o", "--max-tokens=10"])).unwrap();
    assert_eq!(f.config.model.as_deref(), Some("gpt-4o"));
    assert_eq!(f.config.max_tokens, Some(10));
}

#[test]
fn timeout_flags_set_the_transport_bounds() {
    let f = parse_args(&argv(&[
        "--timeout-connect",
        "5",
        "--timeout-response=60",
        "--timeout-idle",
        "90",
    ]))
    .unwrap();
    assert_eq!(f.config.timeout_connect, Some(5));
    assert_eq!(f.config.timeout_response, Some(60));
    assert_eq!(f.config.timeout_idle, Some(90));
}

#[test]
fn a_non_numeric_timeout_is_usage_64() {
    let err = parse_args(&argv(&["--timeout-idle", "soon"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("needs a number"));
}

#[test]
fn unknown_flag_is_usage_64() {
    let err = parse_args(&argv(&["--nope"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("unknown flag"));
}

#[test]
fn missing_value_is_usage_64() {
    let err = parse_args(&argv(&["--model"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("needs a value"));
}

#[test]
fn bad_number_is_usage_64() {
    let err = parse_args(&argv(&["--max-tokens", "lots"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("needs a number"));
}

#[test]
fn a_multi_word_unquoted_prompt_joins_through_eof() {
    // The first operand stops option parsing; everything through EOF is the prompt,
    // operands joined by ONE space (§5.5/§13.7) — so `bz what is 2+2` needs no quotes.
    let f = parse_args(&argv(&["what", "is", "2+2"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("what is 2+2"));
    assert!(f.config.output.is_none());
}

#[test]
fn options_before_the_prompt_are_honored_but_after_it_are_inert_text() {
    // Options-before-prompt (POSIX Guideline 9): `--json` BEFORE the prompt selects
    // JSON; the SAME flag AFTER the prompt starts is inert prompt text.
    let before = parse_args(&argv(&["--json", "q"])).unwrap();
    assert_eq!(before.config.output, Some(OutMode::Ndjson));
    assert_eq!(before.prompt.as_deref(), Some("q"));
    let after = parse_args(&argv(&["q", "--json"])).unwrap();
    assert_eq!(after.prompt.as_deref(), Some("q --json"));
    assert!(after.config.output.is_none(), "post-prompt flag is text");
}

#[test]
fn dashes_after_the_prompt_starts_are_inert_text() {
    // Once the prompt begins, no token is an option — a `-`/`--`/word after it is all
    // joined into the prompt verbatim, never parsed (so no unknown-flag error either).
    let f = parse_args(&argv(&["hello", "--nope", "--", "-x"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("hello --nope -- -x"));
    assert!(f.config.output.is_none());
}

#[test]
fn double_dash_ends_option_parsing() {
    // After `--`, a `-`-leading word is the prompt, not a flag.
    let f = parse_args(&argv(&["--", "--json"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--json"));
    assert!(f.config.output.is_none());
}

#[test]
fn double_dash_tail_joins_through_eof() {
    // The `--` tail is the prompt through EOF, joined by one space — a leading-dash
    // multi-word prompt is reachable only via the options terminator.
    let f = parse_args(&argv(&["--", "--weird", "and", "more"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--weird and more"));
}

#[test]
fn bare_double_dash_leaves_no_positional() {
    // `bz --` with nothing after it: an empty tail is no prompt at all, so the run
    // falls through to the stdin/bare path (prompt stays None).
    let f = parse_args(&argv(&["--"])).unwrap();
    assert!(f.prompt.is_none());
}

#[test]
fn lone_dash_is_a_positional() {
    let f = parse_args(&argv(&["-"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("-"));
}
