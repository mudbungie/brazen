//! Flag parsing (arch §5.5, §5.9): every recognized flag sets the flag-layer
//! config or a pre-resolve field; usage errors (exit 64) are surfaced, never
//! silently absorbed. The argv GRAMMAR (prompt boundary, `--`) lives in
//! `cli_args_prompt`. Pure over a `&[String]`, so no process argv is touched.

use crate::{parse_args, Content, OutMode, ReasoningEffort};

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
fn raw_directional_value_grammar() {
    // Bare `--raw` and `--raw=both` set ONLY the OUTPUT axis (`OutMode::Raw`); the input
    // axis is left `None` to DERIVE from `output` at resolve (§5.10.2), so both spell BOTH.
    for spelling in ["--raw", "--raw=both"] {
        let f = parse_args(&argv(&[spelling])).unwrap();
        assert_eq!(f.config.output, Some(OutMode::Raw), "{spelling} output");
        assert_eq!(f.config.raw_in, None, "{spelling} raw_in");
    }
    // `--raw=in` sets ONLY the input axis — no `OutMode` change, so it composes with the
    // `--text`/`--json` projections (the canonical-out response half).
    let f = parse_args(&argv(&["--raw=in"])).unwrap();
    assert_eq!(f.config.output, None);
    assert_eq!(f.config.raw_in, Some(true));
    // `--raw=out` sets the OUTPUT axis AND pins the input axis normal (constructor runs).
    let f = parse_args(&argv(&["--raw=out"])).unwrap();
    assert_eq!(f.config.output, Some(OutMode::Raw));
    assert_eq!(f.config.raw_in, Some(false));
    // The `OutMode` last-wins fold reaches every raw spelling that sets it: `--raw=out
    // --json` ⇒ json (raw-out lost), but `--raw=in` (input-axis only) survives `--json`.
    let f = parse_args(&argv(&["--raw=out", "--json"])).unwrap();
    assert_eq!(f.config.output, Some(OutMode::Ndjson));
    assert_eq!(f.config.raw_in, Some(false));
    let f = parse_args(&argv(&["--raw=in", "--json"])).unwrap();
    assert_eq!(f.config.output, Some(OutMode::Ndjson));
    assert_eq!(f.config.raw_in, Some(true));
    // An unknown value is a usage error (64) naming the accepted spellings.
    let e = parse_args(&argv(&["--raw=sideways"])).unwrap_err();
    assert_eq!(e.exit_code(), 64);
    assert!(e.message.contains("in`/`out`/`both"), "{}", e.message);
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
    // `--skill` is the third discovery probe — its own pre-resolve bit, no alias.
    let f = parse_args(&argv(&["--skill"])).unwrap();
    assert!(f.skill, "--skill should set skill");
    assert!(!f.help && !f.version);
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
        "--base-url",
        "http://localhost:8080",
        "--config",
        "/tmp/cfg.toml",
    ]))
    .unwrap();
    assert_eq!(f.config.provider.as_deref(), Some("anthropic"));
    assert_eq!(f.config.model.as_deref(), Some("claude"));
    // `--base-url` is the host-override scalar (config §4.5), not a row field.
    assert_eq!(f.config.base_url.as_deref(), Some("http://localhost:8080"));
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
fn timeout_flag_sets_the_transport_bound() {
    // The one silence budget (both value forms); the three old `--timeout-*` flags
    // collapsed to this (arch §13.15).
    let f = parse_args(&argv(&["--timeout", "90"])).unwrap();
    assert_eq!(f.config.timeout, Some(90));
    let g = parse_args(&argv(&["--timeout=45"])).unwrap();
    assert_eq!(g.config.timeout, Some(45));
}

#[test]
fn a_non_numeric_timeout_is_usage_64() {
    let err = parse_args(&argv(&["--timeout", "soon"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("needs a number"));
}

#[test]
fn the_removed_per_phase_timeout_flags_are_unknown_flags_64() {
    // The collapse to one `--timeout` (arch §13.15) really deletes the three: each
    // old spelling now falls through to the unknown-flag arm (64), proving they are
    // gone — no residual per-phase knowledge anywhere in the parser.
    for old in ["--timeout-connect", "--timeout-response", "--timeout-idle"] {
        let err = parse_args(&argv(&[old, "5"])).unwrap_err();
        assert_eq!(err.exit_code(), 64, "{old} should be unknown");
        assert!(
            err.message.contains("unknown flag"),
            "{old}: {}",
            err.message
        );
    }
}

#[test]
fn reasoning_flag_parses_each_effort_both_forms() {
    // The portable request knob (§5.3): a SEPARATE flag from display-only `--thinking`.
    for (word, effort) in [
        ("low", ReasoningEffort::Low),
        ("medium", ReasoningEffort::Medium),
        ("high", ReasoningEffort::High),
    ] {
        let space = parse_args(&argv(&["--reasoning", word])).unwrap();
        assert_eq!(space.config.reasoning, Some(effort));
        let eq = parse_args(&argv(&[&format!("--reasoning={word}")])).unwrap();
        assert_eq!(eq.config.reasoning, Some(effort));
    }
    // `--thinking` and `--reasoning` are orthogonal: setting one leaves the other unset.
    let f = parse_args(&argv(&["--reasoning", "high"])).unwrap();
    assert_eq!(f.config.thinking, None);
}

#[test]
fn a_bad_reasoning_value_is_usage_64() {
    let err = parse_args(&argv(&["--reasoning", "extreme"])).unwrap_err();
    assert_eq!(err.exit_code(), 64);
    assert!(err.message.contains("low|medium|high"));
    assert!(err.message.contains("extreme"));
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
