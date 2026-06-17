//! Per-row `body_defaults` resolution (config §4.1): gen scalars fold into the
//! resolved typed fields beneath flag/env/file, non-gen keys merge into the
//! resolved `extra` (row over top-level), and a malformed gen scalar is a
//! `BadValue` (→78). The `fill_absent` seeding of the passthrough lives in the
//! `config_fill` suite; this one owns the `into_resolved` fold itself.

use brazen::{ConfigError, PartialConfig, ResolvedConfig};
use serde_json::json;

fn resolve(flags: PartialConfig, file: PartialConfig) -> Result<ResolvedConfig, ConfigError> {
    flags
        .or(file)
        .or(PartialConfig::default())
        .into_resolved(Some("m"))
}

/// A complete `openai_chat` row named `p` carrying the given inline `body_defaults`
/// contents — selected by `--provider p`, completed by its own fields.
fn row_with(body_defaults: &str) -> PartialConfig {
    brazen::parse_config(&format!(
        "[[provider]]\nname = \"p\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nbody_defaults = {{ {body_defaults} }}\n"
    ))
    .unwrap()
}

fn select_p() -> PartialConfig {
    PartialConfig {
        provider: Some("p".into()),
        ..Default::default()
    }
}

#[test]
fn gen_scalars_fold_into_the_typed_fields_and_a_flag_beats_them() {
    let cfg = resolve(
        select_p(),
        row_with("max_tokens = 100, temperature = 0.5, top_p = 0.9, stream = true"),
    )
    .unwrap();
    assert_eq!(cfg.max_tokens, Some(100));
    assert_eq!(cfg.temperature, Some(0.5));
    assert_eq!(cfg.top_p, Some(0.9));
    assert_eq!(cfg.stream, Some(true));
    // A flag beats the row default for the same field (config §4.1 precedence).
    let flags = PartialConfig {
        max_tokens: Some(7),
        stream: Some(false),
        ..select_p()
    };
    let cfg2 = resolve(flags, row_with("max_tokens = 100, stream = true")).unwrap();
    assert_eq!(cfg2.max_tokens, Some(7));
    assert_eq!(cfg2.stream, Some(false));
}

#[test]
fn passthrough_merges_into_extra_over_a_top_level_key() {
    // A non-gen key (`store`) becomes resolved `extra`; the row wins over a top-level
    // `extra` key of the same name (more specific), and a top-level-only key survives.
    let mut file = row_with("store = false, seed = 7");
    file.extra.insert("store".into(), json!(true)); // top-level (loses to the row)
    file.extra.insert("top_only".into(), json!(1)); // top-level only (survives)
    let cfg = resolve(select_p(), file).unwrap();
    assert_eq!(cfg.extra.get("store"), Some(&json!(false))); // row wins
    assert_eq!(cfg.extra.get("seed"), Some(&json!(7)));
    assert_eq!(cfg.extra.get("top_only"), Some(&json!(1)));
}

#[test]
fn a_malformed_gen_scalar_is_a_bad_value() {
    // Wrong JSON type or out of range for a gen scalar surfaces at resolve (config §4.1, §7).
    for bd in [
        "max_tokens = 0",
        "max_tokens = \"lots\"",
        "temperature = true",
        "top_p = \"x\"",
        "stream = 1",
    ] {
        let err = resolve(select_p(), row_with(bd)).unwrap_err();
        assert!(matches!(err, ConfigError::BadValue { .. }), "bd = {bd}");
    }
}
