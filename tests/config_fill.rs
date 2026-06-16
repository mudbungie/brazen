//! `fill_absent`, the resolved queries (`raw`, `effective_max_tokens`), and the
//! embedded `defaults.toml` validity (config §4, §4.1, §3.5).

use brazen::{
    defaults, fill_absent, resolve, AuthId, CanonicalRequest, EnvSnapshot, OutMode, PartialConfig,
    ProtocolId, ResolvedConfig,
};

fn resolved(flags: PartialConfig, model: &str) -> ResolvedConfig {
    let r = CanonicalRequest {
        model: model.into(),
        ..Default::default()
    };
    resolve(
        flags,
        &EnvSnapshot::default(),
        PartialConfig::default(),
        defaults(),
        Some(&r),
    )
    .unwrap()
}

fn select(provider: &str) -> PartialConfig {
    PartialConfig {
        provider: Some(provider.into()),
        ..Default::default()
    }
}

#[test]
fn embedded_defaults_carry_the_anthropic_and_openai_rows() {
    let d = defaults();
    let anthropic = d.providers.get("anthropic").unwrap();
    assert_eq!(anthropic.protocol, Some(ProtocolId::AnthropicMessages));
    assert_eq!(anthropic.auth, Some(AuthId::ApiKey));
    assert_eq!(anthropic.default_max_tokens, Some(4096));
    let openai = d.providers.get("openai").unwrap();
    assert_eq!(openai.protocol, Some(ProtocolId::OpenAiChat));
    assert_eq!(openai.auth, Some(AuthId::Bearer));
    assert_eq!(openai.default_max_tokens, None);
}

#[test]
fn raw_is_a_query_over_the_output_mode() {
    let raw = resolved(
        PartialConfig {
            output: Some(OutMode::Raw),
            ..select("anthropic")
        },
        "m",
    );
    assert!(raw.raw());
    let text = resolved(select("anthropic"), "m");
    assert!(!text.raw());
    assert_eq!(text.output, OutMode::Text); // default projection
}

#[test]
fn effective_max_tokens_prefers_config_then_the_row_default() {
    // Config value wins over the row default.
    let with_cfg = resolved(
        PartialConfig {
            max_tokens: Some(100),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(with_cfg.effective_max_tokens(), Some(100));
    // No config value: the anthropic row default fills it.
    let row_default = resolved(select("anthropic"), "m");
    assert_eq!(row_default.effective_max_tokens(), Some(4096));
    // A provider with no row default stays None (omitted from the wire).
    let none = resolved(select("openai"), "m");
    assert_eq!(none.effective_max_tokens(), None);
}

#[test]
fn fill_absent_fills_only_what_the_request_omits() {
    let cfg = resolved(
        PartialConfig {
            temperature: Some(0.3),
            top_p: Some(0.8),
            ..select("anthropic")
        },
        "claude-x",
    );
    let mut req = CanonicalRequest::default(); // empty model, all gen fields None
    fill_absent(&mut req, &cfg);
    assert_eq!(req.model, "claude-x"); // empty -> filled
    assert_eq!(req.max_tokens, Some(4096)); // row default via effective_max_tokens
    assert_eq!(req.temperature, Some(0.3));
    assert_eq!(req.top_p, Some(0.8));
}

#[test]
fn fill_absent_leaves_request_present_fields_untouched() {
    let cfg = resolved(
        PartialConfig {
            max_tokens: Some(50),
            temperature: Some(0.3),
            top_p: Some(0.8),
            ..select("anthropic")
        },
        "claude-x",
    );
    let mut req = CanonicalRequest {
        model: "mine".into(),
        max_tokens: Some(7),
        temperature: Some(0.9),
        top_p: Some(0.1),
        ..Default::default()
    };
    fill_absent(&mut req, &cfg);
    assert_eq!(req.model, "mine"); // present -> untouched
    assert_eq!(req.max_tokens, Some(7));
    assert_eq!(req.temperature, Some(0.9));
    assert_eq!(req.top_p, Some(0.1));
}
