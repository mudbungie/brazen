//! The fold + `into_resolved` (config §3, §7): the fold, alias-based routing, and
//! every surfaced `Config` error. Family-prefix routing lives in `config_route`;
//! both share the helpers in [`config_support`].

use crate::tests::config_support::{file, no_env, req, resolve, ANTHROPIC_ROW};

use crate::{AuthId, ConfigError, EnvSnapshot, PartialConfig, ProtocolId, Timeouts};

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
        crate::defaults(),
        Some(&req("claude-3-7")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.provider.protocol, ProtocolId::AnthropicMessages);
    assert_eq!(cfg.provider.auth, AuthId::ApiKey);
    assert_eq!(cfg.model, "claude-3-7"); // unaliased -> passthrough
    assert_eq!(cfg.max_tokens, Some(4096)); // row body_default folded at resolve (§4.1)
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
fn a_given_model_owned_by_no_row_is_no_provider() {
    // A model IS given but matches zero rows (no alias, no prefix): a partial cannot
    // PICK a provider (arch §4.3 `bz -m opus "q"`), so it is still NoProvider — only
    // the NO-model case defaults (next test).
    let unmatched = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("gpt-4")),
    )
    .unwrap_err();
    assert_eq!(unmatched, ConfigError::NoProvider);
}

#[test]
fn no_provider_and_no_model_defaults_to_the_first_row() {
    // The zero-config `bz "q"`: nothing named, no model. Resolution defaults to the
    // FIRST-DECLARED provider row (arch §4.3) with an empty wire model — `select_model`'s
    // empty seed then takes the first cached model in `serve`. NOT NoProvider.
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.model, "");
}

#[test]
fn no_provider_no_model_and_an_empty_table_is_still_no_provider() {
    // The lone residue of NoProvider on the no-model path: an EMPTY provider table —
    // there is no first row to default to.
    let err = resolve(
        PartialConfig::default(),
        &no_env(),
        PartialConfig::default(),
        PartialConfig::default(),
        None,
    )
    .unwrap_err();
    assert_eq!(err, ConfigError::NoProvider);
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
        crate::defaults(),
        Some(&req("m")),
    )
    .unwrap_err();
    assert!(matches!(err, ConfigError::BadValue { .. }));
}
