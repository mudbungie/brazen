//! Control flags & the total bare-prompt namespace (arch §5.10.1): control ops are
//! `--login`/`--list-models` flags, never `argv[0]` verbs, so a leading bare word is
//! ALWAYS a prompt; combining two control ops is a usage error (64) unless a probe
//! (`--help`/`--version`) answers first; and `route` keys on the control flag, never
//! on `argv[0]`. Pure over a `&[String]`, so no process argv is touched.

use crate::{parse_args, route, Route};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

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
    let c = parse_args(&argv(&["--count-tokens"])).unwrap();
    assert!(c.count_tokens);
    assert!(!c.list_models);
    assert!(!c.login);
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
        ["--count-tokens", "--list-models"],
        ["--count-tokens", "--dump-config"],
        ["--count-tokens", "--login"],
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
    assert!(matches!(
        route(&argv(&["--count-tokens"])),
        Route::CountTokens
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
