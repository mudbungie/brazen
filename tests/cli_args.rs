//! Flag parsing (arch §5.5, §5.9): every recognized flag sets the flag-layer
//! config or a pre-resolve field; usage errors (exit 64) are surfaced, never
//! silently absorbed. Pure over a `&[String]`, so no process argv is touched.

use brazen::{parse_args, OutMode};

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
fn value_flags_equals_form() {
    let f = parse_args(&argv(&["--model=gpt-4o", "--max-tokens=10"])).unwrap();
    assert_eq!(f.config.model.as_deref(), Some("gpt-4o"));
    assert_eq!(f.config.max_tokens, Some(10));
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
fn two_positionals_is_usage_64() {
    let err = parse_args(&argv(&["one", "two"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("one positional"));
}

#[test]
fn double_dash_ends_option_parsing() {
    // After `--`, a `-`-leading word is the prompt, not a flag.
    let f = parse_args(&argv(&["--", "--json"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--json"));
    assert!(f.config.output.is_none());
}

#[test]
fn lone_dash_is_a_positional() {
    let f = parse_args(&argv(&["-"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("-"));
}
