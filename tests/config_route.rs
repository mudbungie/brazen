//! Model→provider routing by owned model-id FAMILY (arch §4.3, config §7): a row
//! claims a family via `model_prefixes`, so an unmistakable wire id no alias could
//! enumerate routes with no `--provider` — the bl-72dc ergonomic. The ambiguity
//! guard is unchanged; alias routing and the errors live in `config_resolve`.

mod config_support;
use config_support::{file, no_env, req, resolve, ANTHROPIC_ROW};

use brazen::{ConfigError, PartialConfig};

#[test]
fn an_owned_family_prefix_routes_with_no_provider_named() {
    // The headline ergonomic: a versioned wire id no alias could ever enumerate
    // routes by the family prefix its embedded row OWNS — `--provider` droppable.
    for (model, provider) in [
        ("claude-haiku-4-5-20251001", "anthropic"),
        ("gpt-5.4", "openai"),
        ("o3-mini", "openai"),
        ("gemini-2.0-flash", "google"),
        ("mistral-large-latest", "mistral"),
    ] {
        let cfg = resolve(
            PartialConfig::default(),
            &no_env(),
            PartialConfig::default(),
            brazen::defaults(),
            Some(&req(model)),
        )
        .unwrap();
        assert_eq!(cfg.provider.name, provider, "routing {model}");
        assert_eq!(cfg.model, model); // unaliased wire id passes through verbatim
    }
}

#[test]
fn two_rows_claiming_one_family_prefix_stay_ambiguous() {
    // The ambiguous-match guard is unchanged for prefix ownership: two rows that
    // both claim a family is a `Config` (78), never a silent pick (arch §4.3).
    let two = file(
        "[[provider]]\nname = \"a\"\nbase_url = \"a\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nmodel_prefixes = [\"shared-\"]\n[[provider]]\nname = \"b\"\nbase_url = \"b\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nmodel_prefixes = [\"shared-\"]\n",
    );
    let err = resolve(
        PartialConfig::default(),
        &no_env(),
        two,
        PartialConfig::default(),
        Some(&req("shared-7")),
    )
    .unwrap_err();
    match err {
        ConfigError::AmbiguousModel { model, providers } => {
            assert_eq!(model, "shared-7");
            assert_eq!(providers, vec!["a".to_string(), "b".to_string()]);
        }
        other => panic!("expected AmbiguousModel, got {other:?}"),
    }
}

#[test]
fn a_model_matching_no_family_prefix_still_needs_a_provider() {
    // A row owning `claude-` does NOT own an unrelated id: it matches no row and
    // so still requires explicit `--provider` (arch §4.3 routing, not substitution).
    let err = resolve(
        PartialConfig::default(),
        &no_env(),
        file("[[provider]]\nname = \"anthropic\"\nbase_url = \"u\"\nprotocol = \"anthropic_messages\"\nauth = \"api_key\"\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\nmodel_prefixes = [\"claude-\"]\n"),
        PartialConfig::default(),
        Some(&req("gpt-4")),
    )
    .unwrap_err();
    assert_eq!(err, ConfigError::NoProvider);
}

#[test]
fn the_probe_query_is_false_for_an_owned_id_in_the_routed_case() {
    // The owned-vs-probe query (model-discovery §5.1), the ROUTED case (no
    // `--provider`): a model only reaches a provider by OWNING a row here (a partial
    // that owns nothing is `NoProvider`, §7), so every routed model is owned and
    // `probe` is false — `cfg.model` is the final wire id, `serve` is one round-trip.
    // A prefix-owned full id passes through verbatim; an exact alias is substituted.
    let owned_prefix = resolve(
        PartialConfig::default(),
        &no_env(),
        PartialConfig::default(),
        brazen::defaults(),
        Some(&req("claude-haiku-4-5-20251001")), // prefix-owned full wire id
    )
    .unwrap();
    assert!(!owned_prefix.probe);
    assert_eq!(owned_prefix.model, "claude-haiku-4-5-20251001"); // verbatim wire id

    let exact_alias = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("sonnet")), // an exact `model_aliases` key
    )
    .unwrap();
    assert!(!exact_alias.probe);
    assert_eq!(exact_alias.model, "claude-3-5-sonnet"); // substituted wire id
}

#[test]
fn the_probe_query_covers_the_explicit_provider_case_route_does_not_check() {
    // The §5.1 EXTENSION: `route` does not check ownership when a provider is NAMED,
    // so the probe query is what distinguishes an owned id from a partial/absent
    // THERE — the case `route` ignores. A NAMED provider with an id it OWNS is false;
    // with a partial, an absent model, or an id the row owns nothing of, it is true.
    let owned = resolve(
        PartialConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        },
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("sonnet")), // the row OWNS this (exact alias)
    )
    .unwrap();
    assert!(!owned.probe);
    assert_eq!(owned.model, "claude-3-5-sonnet");

    // A partial of the alias: NAMED routing accepts it, but the row owns no such id,
    // so it is a SEED `serve` expands (verbatim — alias substitution is identity for
    // an unowned string, config §7).
    let partial = resolve(
        PartialConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        },
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("son")), // not an exact alias key, no `model_prefixes` to own it
    )
    .unwrap();
    assert!(partial.probe);
    assert_eq!(partial.model, "son"); // the partial verbatim is the SEED

    // Absent model + a NAMED provider: routing succeeds, the seed is the empty `""`.
    let absent = resolve(
        PartialConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        },
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        None,
    )
    .unwrap();
    assert!(absent.probe);
    assert_eq!(absent.model, ""); // the empty SEED
}
