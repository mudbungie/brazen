//! `partial_from_env` and `config_path` (config §3.4, §5): pure projections of
//! an injected snapshot — no process environment, no temp files.

use std::path::PathBuf;

use brazen::{config_path, partial_from_env, EnvSnapshot, OutMode, Secret};

fn env(pairs: &[(&str, &str)]) -> EnvSnapshot {
    EnvSnapshot(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

#[test]
fn projects_each_modeled_variable() {
    let cfg = partial_from_env(&env(&[
        ("BRAZEN_PROVIDER", "anthropic"),
        ("BRAZEN_MODEL", "sonnet"),
        ("BRAZEN_MAX_TOKENS", "2048"),
        ("BRAZEN_TEMPERATURE", "0.4"),
        ("BRAZEN_TOP_P", "0.9"),
        ("BRAZEN_STREAM", "true"),
        ("BRAZEN_OUTPUT", "ndjson"),
        ("BRAZEN_THINKING", "true"),
        ("BRAZEN_TIMEOUT_CONNECT", "5"),
        ("BRAZEN_TIMEOUT_RESPONSE", "60"),
        ("BRAZEN_TIMEOUT_IDLE", "90"),
    ]))
    .unwrap();
    assert_eq!(cfg.provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.model.as_deref(), Some("sonnet"));
    assert_eq!(cfg.max_tokens, Some(2048));
    assert_eq!(cfg.temperature, Some(0.4));
    assert_eq!(cfg.top_p, Some(0.9));
    assert_eq!(cfg.stream, Some(true));
    assert_eq!(cfg.output, Some(OutMode::Ndjson));
    assert_eq!(cfg.thinking, Some(true));
    assert_eq!(cfg.timeout_connect, Some(5));
    assert_eq!(cfg.timeout_response, Some(60));
    assert_eq!(cfg.timeout_idle, Some(90));
}

#[test]
fn empty_env_is_the_identity_partial() {
    let cfg = partial_from_env(&EnvSnapshot::default()).unwrap();
    assert_eq!(cfg, brazen::PartialConfig::default());
    // EnvSnapshot is Clone + Debug + PartialEq.
    let e = env(&[("A", "b")]);
    assert_eq!(e.clone(), e);
    assert!(!format!("{e:?}").is_empty());
    assert_eq!(e.get("A"), Some("b"));
    assert_eq!(e.get("Z"), None);
}

#[test]
fn brazen_api_key_outranks_the_anthropic_alias() {
    let cfg = partial_from_env(&env(&[
        ("BRAZEN_API_KEY", "brazen"),
        ("ANTHROPIC_API_KEY", "anthropic"),
    ]))
    .unwrap();
    assert_eq!(cfg.api_key.as_ref().map(Secret::expose), Some("brazen"));
}

#[test]
fn the_anthropic_alias_is_accepted_when_brazen_is_absent() {
    let cfg = partial_from_env(&env(&[("ANTHROPIC_API_KEY", "anthropic")])).unwrap();
    assert_eq!(cfg.api_key.as_ref().map(Secret::expose), Some("anthropic"));
}

#[test]
fn brazen_config_is_not_a_resolved_field() {
    // $BRAZEN_CONFIG selects which file, not a value of the resolved config.
    let cfg = partial_from_env(&env(&[("BRAZEN_CONFIG", "/etc/brazen.toml")])).unwrap();
    assert_eq!(cfg, brazen::PartialConfig::default());
}

#[test]
fn unparseable_env_scalars_are_bad_values() {
    for (key, val) in [
        ("BRAZEN_MAX_TOKENS", "lots"),
        ("BRAZEN_TEMPERATURE", "warm"),
        ("BRAZEN_TOP_P", "high"),
        ("BRAZEN_STREAM", "yes"),
        ("BRAZEN_OUTPUT", "xml"),
        ("BRAZEN_THINKING", "maybe"),
        ("BRAZEN_TIMEOUT_CONNECT", "soon"),
        ("BRAZEN_TIMEOUT_RESPONSE", "later"),
        ("BRAZEN_TIMEOUT_IDLE", "never"),
    ] {
        let err = partial_from_env(&env(&[(key, val)])).unwrap_err();
        assert!(format!("{err}").contains(key), "{key} should surface");
    }
}

#[test]
fn config_path_prefers_the_explicit_flag() {
    let path = config_path(
        Some(PathBuf::from("/flag.toml")),
        &env(&[("BRAZEN_CONFIG", "/env.toml")]),
    );
    assert_eq!(path, PathBuf::from("/flag.toml"));
}

#[test]
fn config_path_falls_back_to_brazen_config_then_xdg() {
    assert_eq!(
        config_path(None, &env(&[("BRAZEN_CONFIG", "/env.toml")])),
        PathBuf::from("/env.toml")
    );
    assert_eq!(
        config_path(None, &env(&[("XDG_CONFIG_HOME", "/xdg")])),
        PathBuf::from("/xdg/brazen/config.toml")
    );
    assert_eq!(
        config_path(None, &env(&[("HOME", "/home/me")])),
        PathBuf::from("/home/me/.config/brazen/config.toml")
    );
    // Neither XDG nor HOME: a relative .config path.
    assert_eq!(
        config_path(None, &EnvSnapshot::default()),
        PathBuf::from(".config/brazen/config.toml")
    );
}
