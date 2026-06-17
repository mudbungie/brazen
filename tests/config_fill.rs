//! `fill_absent`, the resolved gen defaults (incl. the row `body_defaults` fold),
//! and the embedded `defaults.toml` validity (config §4, §4.1, §3.5).

use brazen::{
    defaults, fill_absent, AuthId, CanonicalRequest, Content, OutMode, PartialConfig, ProtocolId,
    ResolvedConfig,
};
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
    assert_eq!(req.max_tokens, Some(4096)); // row body_default folded into cfg.max_tokens
    assert_eq!(req.temperature, Some(0.3));
    assert_eq!(req.top_p, Some(0.8));
}

#[test]
fn fill_absent_propagates_stream_from_config_but_a_request_setting_wins() {
    // `--stream`/`BRAZEN_STREAM`/file resolve into `cfg.stream` and seed a request
    // that omits it — the regression: previously the flag never reached the wire,
    // so every live SSE 2xx decoded to `premature upstream EOF` (config §4).
    let cfg = resolved(
        PartialConfig {
            stream: Some(true),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(cfg.stream, Some(true)); // carried by into_resolved

    let mut omitted = CanonicalRequest::default(); // stream: None
    fill_absent(&mut omitted, &cfg);
    assert_eq!(omitted.stream, Some(true)); // absent -> filled from config

    // A request that sets `stream` wins as-is, even against a streaming config.
    let mut present = CanonicalRequest {
        stream: Some(false),
        ..Default::default()
    };
    fill_absent(&mut present, &cfg);
    assert_eq!(present.stream, Some(false)); // present -> untouched

    // Neither set: stays absent (encoders read `unwrap_or(false)`).
    let mut bare = CanonicalRequest::default();
    fill_absent(&mut bare, &resolved(select("anthropic"), "m"));
    assert_eq!(bare.stream, None);
}

#[test]
fn fill_absent_supplies_the_config_system_prompt_when_the_request_omits_it() {
    let cfg = resolved(
        PartialConfig {
            system: Some(vec![Content::Text("be terse".into())]),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(cfg.system, Some(vec![Content::Text("be terse".into())])); // carried by into_resolved
    let mut req = CanonicalRequest::default(); // no system
    fill_absent(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text("be terse".into())])); // absent -> filled
}

#[test]
fn fill_absent_leaves_a_request_system_prompt_untouched() {
    let cfg = resolved(
        PartialConfig {
            system: Some(vec![Content::Text("config".into())]),
            ..select("anthropic")
        },
        "m",
    );
    let mut req = CanonicalRequest {
        system: Some(vec![Content::Text("request".into())]),
        ..Default::default()
    };
    fill_absent(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text("request".into())])); // present -> untouched
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

#[test]
fn fill_absent_seeds_config_passthrough_into_req_extra() {
    // A row's non-gen `body_defaults` becomes `cfg.extra`; `fill_absent` seeds it into
    // `req.extra` BENEATH the request's own keys (config §4.1) — the live encode seam.
    let file = brazen::parse_config(
        "[[provider]]\nname = \"p\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nbody_defaults = { store = false, seed = 7 }\n",
    )
    .unwrap();
    let cfg = PartialConfig {
        provider: Some("p".into()),
        ..Default::default()
    }
    .or(file)
    .or(defaults())
    .into_resolved(Some("m"))
    .unwrap();
    assert_eq!(cfg.extra.get("store"), Some(&json!(false))); // non-gen passthrough resolved
                                                             // The request brings its own `store` (wins); `seed` is absent (config seeds it).
    let mut req: CanonicalRequest =
        serde_json::from_value(json!({"model":"m","messages":[],"store":true})).unwrap();
    fill_absent(&mut req, &cfg);
    assert_eq!(req.extra.get("store"), Some(&json!(true))); // request's own key wins
    assert_eq!(req.extra.get("seed"), Some(&json!(7))); // config fills the gap
}
