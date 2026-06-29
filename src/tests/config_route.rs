//! Model→provider routing by owned model-id FAMILY (arch §4.3, config §7): a row
//! claims a family via `model_prefixes`, so an unmistakable wire id no alias could
//! enumerate routes with no `--provider` — the bl-72dc ergonomic. The ambiguity
//! guard is unchanged; alias routing and the errors live in `config_resolve`.

use crate::tests::config_support::{
    file, no_env, req, resolve, ANTHROPIC_ROW, PREFIX_LESS_ROW, PREFIX_ROW,
};

use crate::{ConfigError, PartialConfig};

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
            crate::defaults(),
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
fn resolution_produces_a_model_seed_routing_only_no_cache_lookup() {
    // The probe is DISSOLVED (model-discovery §5): resolution does routing + alias
    // substitution ONLY and computes NOTHING about the cache — `model_from_cache` is
    // false until `serve`'s cache lookup runs. The result is a SEED `select_model`
    // places: a prefix-owned full id passes through verbatim, an exact alias is
    // substituted to its wire id.
    let owned_prefix = resolve(
        PartialConfig::default(),
        &no_env(),
        PartialConfig::default(),
        crate::defaults(),
        Some(&req("claude-haiku-4-5-20251001")), // prefix-owned full wire id
    )
    .unwrap();
    assert!(!owned_prefix.model_from_cache);
    assert_eq!(owned_prefix.model, "claude-haiku-4-5-20251001"); // verbatim seed

    let exact_alias = resolve(
        PartialConfig::default(),
        &no_env(),
        file(ANTHROPIC_ROW),
        PartialConfig::default(),
        Some(&req("sonnet")), // an exact `model_aliases` key
    )
    .unwrap();
    assert!(!exact_alias.model_from_cache);
    assert_eq!(exact_alias.model, "claude-3-5-sonnet"); // substituted wire id
}

#[test]
fn a_named_provider_carries_its_seed_verbatim_partial_or_absent() {
    // `route` does not check ownership when a provider is NAMED, and resolution no
    // longer expands the model — so an exact alias substitutes, a partial passes
    // through verbatim (the SEED `serve` later matches against the cache), and an
    // absent model is the empty `""` seed. No probe, no cache touch here.
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
    assert_eq!(owned.model, "claude-3-5-sonnet"); // substituted

    // A partial of the alias: NAMED routing accepts it, alias substitution is identity
    // for an unowned string (config §7), so the partial is the verbatim SEED. (Whether
    // it EXPANDS is `serve`'s cache lookup, not resolution's.)
    let partial = resolve(
        PartialConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        },
        &no_env(),
        file(PREFIX_ROW),
        PartialConfig::default(),
        Some(&req("son")), // not an exact alias key → passes through verbatim
    )
    .unwrap();
    assert_eq!(partial.model, "son"); // the partial verbatim is the SEED

    // Absent model + a NAMED provider: routing succeeds, the seed is the empty `""`.
    let absent = resolve(
        PartialConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        },
        &no_env(),
        file(PREFIX_ROW),
        PartialConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(absent.model, ""); // the empty SEED `serve` defaults from the cache
}

#[test]
fn a_prefix_less_row_carries_a_present_model_seed_verbatim() {
    // The bl-3989 motivating case, now DISSOLVED at the root: with no auto-list at all,
    // a prefix-less row (the `openai-responses`/`openai-chatgpt` shape) reached with a
    // fully-qualified `--model` simply carries it as the verbatim SEED — `serve`'s cache
    // lookup is a local FILE read (no `/models` GET can ever fire on the generation
    // path). An absent model is the empty seed. `PREFIX_LESS_ROW` declares no prefixes.
    let present = resolve(
        PartialConfig {
            provider: Some("codex".into()),
            ..Default::default()
        },
        &no_env(),
        file(PREFIX_LESS_ROW),
        PartialConfig::default(),
        Some(&req("gpt-5.4")), // a fully-qualified exact wire id
    )
    .unwrap();
    assert_eq!(present.model, "gpt-5.4"); // verbatim seed

    let absent = resolve(
        PartialConfig {
            provider: Some("codex".into()),
            ..Default::default()
        },
        &no_env(),
        file(PREFIX_LESS_ROW),
        PartialConfig::default(),
        None, // no model → the empty seed → cache default in serve
    )
    .unwrap();
    assert_eq!(absent.model, ""); // the empty SEED
}

#[test]
fn no_provider_no_model_defaults_to_the_first_declared_row() {
    // The zero-config default (arch §4.3): nothing named, no model → the FIRST-DECLARED
    // provider, in config-FILE order — "whatever you find first reading from the top",
    // NOT the alphabetically-first name. The file lists `zeta` before `alpha`; resolution
    // picks `zeta` (declared first), with the empty model seed.
    let two = "[[provider]]\nname = \"zeta\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n[[provider]]\nname = \"alpha\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n";
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(two),
        PartialConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "zeta");
    assert_eq!(cfg.model, "");
}

#[test]
fn a_user_first_row_beats_the_built_in_default_anthropic() {
    // The bl-ac1e regression: with the REAL built-in defaults folded in (anthropic,
    // openai, …), a user config whose first-declared row is `chatty` still defaults to
    // `chatty`, NOT the alphabetically-first built-in `anthropic`. The user's config
    // layer outranks `defaults` in the fold, so its `default_provider` wins (config §4.3).
    let user = "[[provider]]\nname = \"chatty\"\nbase_url = \"u\"\nprotocol = \"openai_responses\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n[[provider]]\nname = \"local\"\nbase_url = \"u\"\nprotocol = \"ollama_chat\"\nauth = \"none\"\n";
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(user),
        crate::defaults(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "chatty");
}
