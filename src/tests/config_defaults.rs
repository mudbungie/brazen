//! The embedded `defaults.toml` validity and the resolved gen defaults (incl.
//! the row `body_defaults` fold) — config §4, §4.1, §3.5. What `fill_absent`
//! does with a resolved config lives in `config_fill`. Per-file harness copy.

use crate::{defaults, AuthId, OutMode, PartialConfig, ProtocolId, ResolvedConfig};
use serde_json::json;

fn resolved(flags: PartialConfig, model: &str) -> ResolvedConfig {
    // The production composition (run/mod.rs): fold then route by request model.
    flags
        .or(PartialConfig::default())
        .or(defaults())
        .into_resolved(Some(model).filter(|m| !m.is_empty()))
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
    assert_eq!(
        anthropic.body_defaults.get("max_tokens"),
        Some(&json!(4096))
    );
    let openai = d.providers.get("openai").unwrap();
    assert_eq!(openai.protocol, Some(ProtocolId::OpenAiChat));
    assert_eq!(openai.auth, Some(AuthId::Bearer));
    assert!(openai.body_defaults.is_empty());
}

#[test]
fn embedded_defaults_carry_the_transport_timeout_floor() {
    // The bin holds no magic timeout constants — the floor is `defaults.toml`,
    // reaching a resolved run as its lowest-precedence layer (config §4).
    let d = defaults();
    assert_eq!(d.timeout_connect, Some(30));
    assert_eq!(d.timeout_response, Some(120));
    assert_eq!(d.timeout_idle, Some(300));
    // And it survives the fold onto the resolved config's `timeouts()` query.
    let cfg = resolved(select("anthropic"), "m");
    assert_eq!(cfg.timeouts().connect, Some(30));
    assert_eq!(cfg.timeouts().response, Some(120));
    assert_eq!(cfg.timeouts().idle, Some(300));
}

#[test]
fn mistral_is_one_row_of_data_reusing_openai_chat_and_bearer() {
    // The severability proof (providers §2): adding Mistral is a row, ZERO Rust —
    // it reuses the SAME OpenAiChat protocol + Bearer auth as the openai row, so it
    // is byte-for-byte the same registry keys with only base_url differing.
    let d = defaults();
    let mistral = d.providers.get("mistral").unwrap();
    assert_eq!(mistral.protocol, Some(ProtocolId::OpenAiChat));
    assert_eq!(mistral.auth, Some(AuthId::Bearer));
    assert_eq!(
        mistral.base_url.as_deref(),
        Some("https://api.mistral.ai/v1")
    );
    assert!(mistral.body_defaults.is_empty()); // Mistral does not require max_tokens
}

#[test]
fn the_new_dialect_rows_select_their_protocols_and_auth() {
    let d = defaults();
    let responses = d.providers.get("openai-responses").unwrap();
    assert_eq!(responses.protocol, Some(ProtocolId::OpenAiResponses));
    assert_eq!(responses.auth, Some(AuthId::Bearer));
    let google = d.providers.get("google").unwrap();
    assert_eq!(google.protocol, Some(ProtocolId::GoogleGenAi));
    assert_eq!(google.auth, Some(AuthId::ApiKey)); // x-goog-api-key is row DATA (§4.1)
    let google_header = google.api_header.as_ref().unwrap();
    assert_eq!(google_header.name, "x-goog-api-key");
    let ollama = d.providers.get("ollama").unwrap();
    assert_eq!(ollama.protocol, Some(ProtocolId::OllamaChat));
    assert_eq!(ollama.auth, Some(AuthId::None)); // keyless local: no cred, no header
    assert!(ollama.api_header.is_none());
}

#[test]
fn a_keyless_none_auth_row_resolves_with_no_api_header() {
    // `auth = "none"` (local Ollama): `complete` requires no `api_header`, the
    // keyless dual of api_key/bearer — resolution succeeds with auth = None.
    let cfg = resolved(
        PartialConfig {
            max_tokens: Some(64),
            ..select("ollama")
        },
        "llama3.2",
    );
    assert_eq!(cfg.provider.name, "ollama");
    assert_eq!(cfg.provider.auth, AuthId::None);
    assert!(cfg.provider.api_header.is_none());
}

#[test]
fn the_output_projection_resolves_through_the_fold() {
    let raw = resolved(
        PartialConfig {
            output: Some(OutMode::Raw),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(raw.output, OutMode::Raw);
    let text = resolved(select("anthropic"), "m");
    assert_eq!(text.output, OutMode::Text); // default projection
    assert!(!text.thinking); // --thinking defaults off
}

#[test]
fn thinking_resolves_to_a_concrete_bool() {
    // The flag flows through the fold to a concrete bool the text sink reads.
    let on = resolved(
        PartialConfig {
            thinking: Some(true),
            ..select("anthropic")
        },
        "m",
    );
    assert!(on.thinking);
}

#[test]
fn body_default_max_tokens_folds_beneath_flag_env_file() {
    // The row's `body_defaults.max_tokens` is folded into `cfg.max_tokens` at resolve,
    // BELOW flag/env/file (config §4.1) — so a config value wins over the row default.
    let with_cfg = resolved(
        PartialConfig {
            max_tokens: Some(100),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(with_cfg.max_tokens, Some(100));
    // No config value: the anthropic row body_default fills it.
    let row_default = resolved(select("anthropic"), "m");
    assert_eq!(row_default.max_tokens, Some(4096));
    // A provider whose row pins nothing stays None (omitted from the wire).
    let none = resolved(select("openai"), "m");
    assert_eq!(none.max_tokens, None);
}
