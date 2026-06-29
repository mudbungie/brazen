//! Flag parsing (arch §5.5, §5.9): every recognized flag sets the flag-layer
//! config or a pre-resolve field; usage errors (exit 64) are surfaced, never
//! silently absorbed. Pure over a `&[String]`, so no process argv is touched.

use crate::{parse_args, route, Content, OutMode, Route};

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

// ===================== control flags & the total namespace (§5.10.1) =====================

#[test]
fn control_flags_set_their_bits_and_browser() {
    let f = parse_args(&argv(&["--login", "--provider", "anthropic", "--browser"])).unwrap();
    assert!(f.login);
    assert!(f.browser);
    assert!(!f.list_models);
    assert_eq!(f.config.provider.as_deref(), Some("anthropic"));
    let g = parse_args(&argv(&["--list-models"])).unwrap();
    assert!(g.list_models);
    assert!(!g.login);
}

#[test]
fn a_leading_bare_word_is_always_a_prompt_never_a_verb() {
    // The frozen namespace rule (§5.10.1): control ops are flags, so the old verbs are
    // now ordinary prompts — `bz "login"`, `bz "list-models"`, `bz "models"` forever.
    for word in ["login", "list-models", "models", "list"] {
        let f = parse_args(&argv(&[word])).unwrap();
        assert_eq!(f.prompt.as_deref(), Some(word), "`{word}` must be a prompt");
        assert!(!f.login, "`{word}` is not --login");
        assert!(!f.list_models, "`{word}` is not --list-models");
    }
}

#[test]
fn a_dash_prompt_after_double_dash_is_text_not_a_control_flag() {
    // `bz -- --login` is the PROMPT "--login", reachable via the opts terminator — the
    // escape that keeps even a leading-dash control-flag spelling usable as a prompt.
    let f = parse_args(&argv(&["--", "--login"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--login"));
    assert!(!f.login);
}

#[test]
fn two_control_ops_combined_is_usage_64() {
    for combo in [
        ["--login", "--list-models"],
        ["--login", "--dump-config"],
        ["--list-models", "--dump-config"],
    ] {
        let err = parse_args(&argv(&combo)).unwrap_err();
        assert_eq!(
            err.exit_code(),
            64,
            "{combo:?} should be mutually exclusive"
        );
        assert!(err.message.contains("mutually exclusive"));
    }
}

#[test]
fn a_probe_wins_over_a_control_op_conflict() {
    // The two probes answer first, so `--help`/`--version` alongside two control ops is
    // NOT the mutual-exclusion error — a probe must respond even with a broken combo.
    let f = parse_args(&argv(&["--login", "--list-models", "--help"])).unwrap();
    assert!(f.help);
    let g = parse_args(&argv(&["--login", "--dump-config", "--version"])).unwrap();
    assert!(g.version);
}

#[test]
fn route_keys_on_the_control_flag_not_argv0() {
    assert!(matches!(
        route(&argv(&["--login", "--provider", "x"])),
        Route::Login
    ));
    assert!(matches!(
        route(&argv(&["--list-models"])),
        Route::ListModels
    ));
    assert!(matches!(route(&argv(&["hello world"])), Route::Run));
    // A bare leading word routes to the data plane as a prompt, never a control plane.
    assert!(matches!(route(&argv(&["login"])), Route::Run));
    // A conflict (parse error) routes to `run`, which re-parses and surfaces the 64.
    assert!(matches!(
        route(&argv(&["--login", "--list-models"])),
        Route::Run
    ));
}
