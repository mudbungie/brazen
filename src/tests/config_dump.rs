//! `--dump-config` (config §6): the same fold minus defaults, secrets redacted,
//! deterministic and round-tripping.

use std::collections::BTreeMap;

use crate::{
    dump_config, parse_config, redact, AuthId, Content, EnvSnapshot, HeaderScheme, HeaderSpec,
    ModelsOverride, OutMode, PartialConfig, PartialProvider, ProtocolId, ReasoningEffort, Secret,
};
use serde_json::json;

fn empty_env() -> EnvSnapshot {
    EnvSnapshot(BTreeMap::new())
}

fn flags_with_scalars() -> PartialConfig {
    PartialConfig {
        model: Some("sonnet".into()),
        // The host-override scalar rides the dump so `--dump-config` shows a
        // `--base-url`/`BRAZEN_BASE_URL` override (config §4.5, §6).
        base_url: Some("http://localhost:8080".into()),
        output: Some(OutMode::Ndjson),
        thinking: Some(true),
        max_tokens: Some(1000),
        temperature: Some(0.5),
        top_p: Some(0.9),
        reasoning: Some(ReasoningEffort::High),
        stream: Some(true),
        timeout: Some(90),
        ..Default::default()
    }
}

#[test]
fn dumps_scalars_deterministically() {
    let out = dump_config(flags_with_scalars(), &empty_env(), PartialConfig::default()).unwrap();
    // Scalars present, order stable (toml::Value orders scalars before tables).
    assert!(out.contains("model = \"sonnet\""));
    assert!(out.contains("base_url = \"http://localhost:8080\"")); // the host override (config §4.5)
    assert!(out.contains("output = \"ndjson\""));
    assert!(out.contains("thinking = true"));
    assert!(out.contains("max_tokens = 1000"));
    assert!(out.contains("temperature = 0.5"));
    assert!(out.contains("top_p = 0.9"));
    assert!(out.contains("reasoning = \"high\""));
    assert!(out.contains("stream = true"));
    assert!(out.contains("timeout = 90"));
    // Byte-stable across runs.
    let again = dump_config(flags_with_scalars(), &empty_env(), PartialConfig::default()).unwrap();
    assert_eq!(out, again);
}

#[test]
fn secret_elides_to_the_inert_sentinel() {
    let flags = PartialConfig {
        api_key: Some(Secret::new("sk-live-supersecret")),
        ..Default::default()
    };
    let out = dump_config(flags, &empty_env(), PartialConfig::default()).unwrap();
    assert!(out.contains("api_key = \"<redacted>\""));
    assert!(!out.contains("supersecret"));
}

#[test]
fn redact_only_touches_a_present_key() {
    let with = redact(PartialConfig {
        api_key: Some(Secret::new("real")),
        ..Default::default()
    });
    assert_eq!(
        with.api_key.as_ref().map(Secret::expose),
        Some("<redacted>")
    );
    let without = redact(PartialConfig::default());
    assert_eq!(without.api_key, None);
}

#[test]
fn omits_the_embedded_defaults() {
    // The dump folds flags/env/file only — the embedded defaults are never an
    // operand, so the floor (anthropic/openai rows) is never baked in (§6).
    let out = dump_config(
        PartialConfig {
            model: Some("m".into()),
            ..Default::default()
        },
        &empty_env(),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("model = \"m\""));
    assert!(!out.contains("anthropic"));
    assert!(!out.contains("[[provider]]"));
}

#[test]
fn dumps_a_selector_when_no_rows() {
    let flags = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    };
    let out = dump_config(flags, &empty_env(), PartialConfig::default()).unwrap();
    assert!(out.contains("provider = \"anthropic\""));
}

#[test]
fn dumps_provider_rows_as_array_of_tables() {
    let file = parse_config(
        "[[provider]]\nname = \"anthropic\"\nbody_defaults = { max_tokens = 8192 }\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\n",
    )
    .unwrap();
    let out = dump_config(PartialConfig::default(), &empty_env(), file).unwrap();
    assert!(out.contains("[[provider]]"));
    assert!(out.contains("name = \"anthropic\""));
    assert!(out.contains("8192")); // the row's body_defaults rides the dump (config §6)
}

#[test]
fn extra_passes_through_but_nulls_drop() {
    let mut extra = serde_json::Map::new();
    extra.insert("safe_prompt".into(), json!(true));
    extra.insert("dead".into(), json!(null));
    let flags = PartialConfig {
        extra,
        ..Default::default()
    };
    let out = dump_config(flags, &empty_env(), PartialConfig::default()).unwrap();
    assert!(out.contains("safe_prompt = true"));
    assert!(!out.contains("dead"));
}

#[test]
fn dump_round_trips_to_an_equal_merged_partial() {
    // merged_without_defaults == parse(dump(merged_without_defaults)) (config §6).
    let mut aliases = BTreeMap::new();
    aliases.insert("sonnet".into(), "claude-3-5-sonnet".into());
    let mut providers = BTreeMap::new();
    providers.insert(
        "anthropic".into(),
        PartialProvider {
            base_url: Some("https://api.anthropic.com".into()),
            protocol: Some(ProtocolId::AnthropicMessages),
            auth: Some(AuthId::ApiKey),
            api_header: Some(HeaderSpec {
                name: "x-api-key".into(),
                scheme: HeaderScheme::Raw,
            }),
            beta_headers: Some(vec![("anthropic-version".into(), "2023-06-01".into())]),
            model_aliases: Some(aliases),
            model_prefixes: Some(vec!["claude-".into()]),
            body_defaults: serde_json::Map::from_iter([("max_tokens".into(), json!(4096))]),
            unsupported_body_keys: Some(vec!["temperature".into(), "top_p".into()]),
            // A `[provider.models]` block rides the dump verbatim (config §4.4, §6) and
            // round-trips — covering ModelsOverride's serialize+deserialize.
            models: Some(ModelsOverride {
                path: Some("/models".into()),
                query: vec![("client_version".into(), "0.0.0".into())],
                array_key: Some("models".into()),
                id_key: Some("slug".into()),
                // A metadata key override rides the dump too (model-discovery §3.2) —
                // round-trips its Serialize+Deserialize alongside the shape keys.
                context_key: Some("context_window".into()),
                ..Default::default()
            }),
            oauth: None,
            ambient: None,
        },
    );
    let merged = PartialConfig {
        model: Some("sonnet".into()),
        // A TOP-LEVEL host-override scalar round-trips through the dump AND the
        // partial_de `base_url` arm, distinct from the row's own `base_url` below
        // (config §4.5, §2.2): both survive the re-parse without colliding.
        base_url: Some("http://proxy.local".into()),
        api_key: Some(Secret::new("<redacted>")),
        output: Some(OutMode::Text),
        max_tokens: Some(2048),
        temperature: Some(0.7),
        top_p: Some(0.95),
        // Round-trips through the dump AND the partial_de `reasoning` arm (config §2.2).
        reasoning: Some(ReasoningEffort::Low),
        stream: Some(false),
        timeout: Some(90),
        system: Some(vec![Content::Text("be terse".into())]),
        providers,
        // A config carrying rows also carries its zero-config default (the first
        // declared row) — the state any parse produces; the dump emits that row
        // first so the round-trip recovers it (config §4.3).
        default_provider: Some("anthropic".into()),
        ..Default::default()
    };
    let dumped = dump_config(merged.clone(), &empty_env(), PartialConfig::default()).unwrap();
    let reparsed = parse_config(&dumped).unwrap();
    assert_eq!(reparsed, merged);
}

#[test]
fn dump_preserves_the_zero_config_default_across_reorder() {
    // The keyed map sorts `alpha` before `zeta`, but the declared default is `zeta`
    // (first-declared). The dump must emit `zeta` FIRST so a re-parse recovers the
    // same `default_provider` — otherwise dumping-and-reusing a config would silently
    // flip the zero-config default to the alphabetically-first row (config §4.3).
    let src = "[[provider]]\nname = \"zeta\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n[[provider]]\nname = \"alpha\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n";
    let parsed = parse_config(src).unwrap();
    assert_eq!(parsed.default_provider.as_deref(), Some("zeta"));
    let dumped = dump_config(parsed.clone(), &empty_env(), PartialConfig::default()).unwrap();
    // `zeta`'s table is emitted ahead of `alpha`'s.
    assert!(
        dumped.find("\"zeta\"").unwrap() < dumped.find("\"alpha\"").unwrap(),
        "default row emitted first:\n{dumped}"
    );
    assert_eq!(
        parse_config(&dumped).unwrap().default_provider.as_deref(),
        Some("zeta")
    );
}

#[test]
fn env_error_propagates_through_dump() {
    let env = EnvSnapshot(BTreeMap::from([(
        "BRAZEN_MAX_TOKENS".into(),
        "not-a-number".into(),
    )]));
    assert!(dump_config(PartialConfig::default(), &env, PartialConfig::default()).is_err());
}
