//! The fold + `into_resolved` (config §3, §7): the fold, model→provider routing
//! as a query over rows, and every surfaced `Config` error.

use brazen::{
    AuthId, CanonicalRequest, ConfigError, EnvSnapshot, PartialConfig, ProtocolId, ResolvedConfig,
    Timeouts,
};

/// The production composition the binary runs (run/mod.rs): project the env,
/// fold `flags > env > file > defaults`, then route by the request model. The
/// request is not a fold operand — only its non-empty model routes (arch §4.3).
fn resolve(
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
    defaults: PartialConfig,
    req: Option<&CanonicalRequest>,
) -> Result<ResolvedConfig, ConfigError> {
    let env = brazen::partial_from_env(env)?;
    let req_model = req.map(|r| r.model.as_str()).filter(|m| !m.is_empty());
    flags.or(env).or(file).or(defaults).into_resolved(req_model)
}

fn no_env() -> EnvSnapshot {
    EnvSnapshot::default()
}

fn req(model: &str) -> CanonicalRequest {
    CanonicalRequest {
        model: model.into(),
        ..Default::default()
    }
}

fn file(toml: &str) -> PartialConfig {
    brazen::parse_config(toml).unwrap()
}

const ANTHROPIC_ROW: &str = "[[provider]]\nname = \"anthropic\"\nbase_url = \"https://api.anthropic.com\"\nprotocol = \"anthropic_messages\"\nauth = \"api_key\"\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\ndefault_max_tokens = 4096\nmodel_aliases = { sonnet = \"claude-3-5-sonnet\" }\n";

#[test]
fn folds_and_routes_through_the_embedded_defaults() {
    let flags = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    };
    let cfg = resolve(
        flags,
        &no_env(),
        PartialConfig::default(),
        brazen::defaults(),
        Some(&req("claude-3-7")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.provider.protocol, ProtocolId::AnthropicMessages);
    assert_eq!(cfg.provider.auth, AuthId::ApiKey);
    assert_eq!(cfg.model, "claude-3-7"); // unaliased -> passthrough
    assert_eq!(cfg.provider.default_max_tokens, Some(4096));
    // ResolvedConfig is Clone + Debug + PartialEq.
    assert_eq!(cfg.clone(), cfg);
    assert!(!format!("{cfg:?}").is_empty());
}

#[test]
fn the_request_model_routes_by_alias_and_substitutes_the_wire_id() {
    // No provider named: the single row whose aliases contain the model wins,
    // and the alias is substituted to the wire id (arch §4.3).
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.model, "claude-3-5-sonnet");
}

#[test]
fn config_model_routes_when_the_request_omits_one() {
    // An empty request model is "absent" -> config supplies it (arch §4.3).
    let flags = PartialConfig {
        model: Some("sonnet".into()),
        ..Default::default()
    };
    let cfg = resolve(
        flags,
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("")),
    )
    .unwrap();
    assert_eq!(cfg.model, "claude-3-5-sonnet");
}

#[test]
fn into_resolved_carries_the_timeouts_and_the_query_projects_them() {
    // Flags supply the bounds; resolution carries each scalar and `timeouts()`
    // projects them onto the seam record `run` stamps on the wire.
    let flags = PartialConfig {
        provider: Some("anthropic".into()),
        timeout_connect: Some(5),
        timeout_response: Some(60),
        timeout_idle: Some(90),
        ..Default::default()
    };
    let cfg = resolve(
        flags,
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("m")),
    )
    .unwrap();
    assert_eq!(cfg.timeout_connect, Some(5));
    assert_eq!(cfg.timeout_response, Some(60));
    assert_eq!(cfg.timeout_idle, Some(90));
    assert_eq!(
        cfg.timeouts(),
        Timeouts {
            connect: Some(5),
            response: Some(60),
            idle: Some(90),
        }
    );
}

#[test]
fn an_ambiguous_model_names_every_match() {
    let two = file(
        "[[provider]]\nname = \"a\"\nbase_url = \"a\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nmodel_aliases = { shared = \"x\" }\n[[provider]]\nname = \"b\"\nbase_url = \"b\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nmodel_aliases = { shared = \"y\" }\n",
    );
    let err = resolve(
        PartialConfig::default(),
        &no_env(),
        two,
        PartialConfig::default(),
        Some(&req("shared")),
    )
    .unwrap_err();
    match err {
        ConfigError::AmbiguousModel { model, providers } => {
            assert_eq!(model, "shared");
            assert_eq!(providers, vec!["a".to_string(), "b".to_string()]);
        }
        other => panic!("expected AmbiguousModel, got {other:?}"),
    }
}

#[test]
fn no_match_and_no_model_are_both_no_provider() {
    // Model matches zero rows.
    let unmatched = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("gpt-4")),
    )
    .unwrap_err();
    assert_eq!(unmatched, ConfigError::NoProvider);
    // No provider named and no model at all.
    let no_model = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        None,
    )
    .unwrap_err();
    assert_eq!(no_model, ConfigError::NoProvider);
}

#[test]
fn a_named_but_undefined_provider_is_unknown() {
    let flags = PartialConfig {
        provider: Some("nope".into()),
        ..Default::default()
    };
    let err = resolve(
        flags,
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("m")),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ConfigError::UnknownProvider {
            name: "nope".into()
        }
    );
}

#[test]
fn a_named_provider_with_no_model_resolves_an_empty_wire_model() {
    let flags = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    };
    let cfg = resolve(
        flags,
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.model, "");
}

#[test]
fn an_incomplete_routed_row_names_the_missing_field() {
    // A user row that no embedded row completes is surfaced per field.
    for (toml, field) in [
        ("[[provider]]\nname = \"p\"\n", "base_url"),
        ("[[provider]]\nname = \"p\"\nbase_url = \"u\"\n", "protocol"),
        (
            "[[provider]]\nname = \"p\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\n",
            "auth",
        ),
        (
            "[[provider]]\nname = \"p\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\n",
            "api_header",
        ),
    ] {
        let flags = PartialConfig {
            provider: Some("p".into()),
            ..Default::default()
        };
        let err = resolve(
            flags,
            &no_env(),
            file(toml),
            PartialConfig::default(),
            Some(&req("m")),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ConfigError::IncompleteProvider {
                name: "p".into(),
                field,
            }
        );
    }
}

#[test]
fn contradictory_scalars_are_bad_values() {
    let select = || PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    };
    for mutate in [
        |c: &mut PartialConfig| c.max_tokens = Some(0),
        |c: &mut PartialConfig| c.temperature = Some(f32::NAN),
        |c: &mut PartialConfig| c.top_p = Some(f32::NAN),
    ] {
        let mut flags = select();
        mutate(&mut flags);
        let err = resolve(
            flags,
            &no_env(),
            file(ANTHROPIC_ROW),
            PartialConfig::default(),
            Some(&req("m")),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::BadValue { .. }));
    }
}

#[test]
fn an_env_error_propagates_through_resolve() {
    let env = EnvSnapshot(std::collections::BTreeMap::from([(
        "BRAZEN_MAX_TOKENS".into(),
        "nope".into(),
    )]));
    let err = resolve(
        PartialConfig::default(),
        &env,
        PartialConfig::default(),
        brazen::defaults(),
        Some(&req("m")),
    )
    .unwrap_err();
    assert!(matches!(err, ConfigError::BadValue { .. }));
}
