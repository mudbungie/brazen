//! The fold + `into_resolved` (config §3, §7): the fold and alias-based routing,
//! incl. every routing-outcome error (ambiguous/unknown/NoProvider). Family-prefix
//! routing lives in `config_route`; row/scalar VALIDATION errors in
//! `config_validate`; all share the helpers in [`config_support`].

use std::collections::BTreeMap;

use crate::tests::config_support::{file, no_env, req, resolve, ANTHROPIC_ROW};

use crate::{
    AuthId, ConfigError, EnvSnapshot, PartialConfig, ProtocolId, ReasoningEffort, Timeouts,
};

/// An `EnvSnapshot` carrying just `BRAZEN_BASE_URL` — the env rung of the host
/// override (config §4.5).
fn env_base_url(url: &str) -> EnvSnapshot {
    EnvSnapshot(BTreeMap::from([("BRAZEN_BASE_URL".into(), url.into())]))
}

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
fn into_resolved_carries_the_timeout_and_the_query_fans_it() {
    // A flag supplies the one silence budget; resolution carries the single scalar
    // and `timeouts()` FANS it onto all three seam budgets (arch §13.15) — the
    // fan-out observed at the record `run` stamps on the wire.
    let flags = PartialConfig {
        provider: Some("anthropic".into()),
        timeout: Some(90),
        // The portable reasoning knob folds onto ResolvedConfig.reasoning like any
        // scalar (providers §6); NOT taken from the row's body_defaults.
        reasoning: Some(ReasoningEffort::High),
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
    assert_eq!(cfg.timeout, Some(90));
    assert_eq!(cfg.reasoning, Some(ReasoningEffort::High));
    // The one value reaches connect, response, AND idle — all equal.
    assert_eq!(
        cfg.timeouts(),
        Timeouts {
            connect: Some(90),
            response: Some(90),
            idle: Some(90),
        }
    );
}

#[test]
fn a_base_url_scalar_overrides_the_resolved_rows_host() {
    // A top-level `base_url` scalar REPLACES the routed row's own `base_url` (config
    // §4.5): same provider, different endpoint. It does NOT create a row — protocol,
    // auth, and routing/alias substitution stay the row's, only the host swaps.
    let src = format!("base_url = \"http://file-host\"\n{ANTHROPIC_ROW}");
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(&src),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.provider.base_url, "http://file-host"); // the override
    assert_eq!(cfg.provider.protocol, ProtocolId::AnthropicMessages); // row's, untouched
    assert_eq!(cfg.provider.auth, AuthId::ApiKey); // row's, untouched
    assert_eq!(cfg.model, "claude-3-5-sonnet"); // alias substitution unaffected
}

#[test]
fn base_url_precedence_is_flag_over_env_over_file_over_row() {
    // The one fold (config §3, §4.5): flag beats env, env beats the file scalar, the
    // file scalar beats the row's own `base_url`. Each rung asserted against the same
    // file (a top-level `base_url` + the anthropic row's `https://api.anthropic.com`).
    let src = format!("base_url = \"http://file-host\"\n{ANTHROPIC_ROW}");

    let flag = PartialConfig {
        base_url: Some("http://flag-host".into()),
        ..Default::default()
    };
    let cfg = resolve(
        flag,
        &env_base_url("http://env-host"),
        file(&src),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.base_url, "http://flag-host"); // flag beats env+file+row

    let cfg = resolve(
        PartialConfig::default(),
        &env_base_url("http://env-host"),
        file(&src),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.base_url, "http://env-host"); // env beats file+row

    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(&src),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.base_url, "http://file-host"); // file scalar beats row
}

#[test]
fn no_base_url_override_leaves_the_rows_own_host() {
    // The `None` defer (config §4.5): with nothing overriding, the routed row's own
    // `base_url` survives — the empty-input general path, not a special case.
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("sonnet")),
    )
    .unwrap();
    assert_eq!(cfg.provider.base_url, "https://api.anthropic.com");
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
