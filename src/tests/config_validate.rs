//! `into_resolved` row/scalar validation (config §3, §7): an incomplete routed
//! row names its missing field, contradictory scalars are `BadValue`, and an env
//! parse error propagates through resolve. Routing outcomes live in
//! `config_resolve`/`config_route`; shared helpers in [`config_support`].

use crate::tests::config_support::{file, no_env, req, resolve, ANTHROPIC_ROW};

use crate::{ConfigError, EnvSnapshot, PartialConfig};

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
