//! Seams: provider rows as data and the id vocabularies (arch §4.2), plus the
//! auth-row `OAuthConfig` and the `AuthCtx`/`ProviderCtx` projections (auth §1.3).

use std::collections::BTreeMap;

use brazen::auth::AuthCtx;
use brazen::{AuthId, HeaderScheme, HeaderSpec, OAuthConfig, ProtocolId, Provider, Secret};
use serde_json::json;

#[test]
fn protocol_id_wire_spelling_round_trips() {
    for (id, wire) in [
        (ProtocolId::OpenAiChat, "openai_chat"),
        (ProtocolId::AnthropicMessages, "anthropic_messages"),
    ] {
        assert_eq!(serde_json::to_value(id).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<ProtocolId>(json!(wire)).unwrap(),
            id
        );
        assert!(!format!("{id:?}").is_empty());
        assert_eq!(id, id); // Copy + PartialEq
    }
}

#[test]
fn auth_id_wire_spelling_round_trips() {
    for (id, wire) in [
        (AuthId::ApiKey, "api_key"),
        (AuthId::Bearer, "bearer"),
        (AuthId::OAuth2, "oauth2"),
    ] {
        assert_eq!(serde_json::to_value(id).unwrap(), json!(wire));
        assert_eq!(serde_json::from_value::<AuthId>(json!(wire)).unwrap(), id);
        assert!(!format!("{id:?}").is_empty());
    }
}

#[test]
fn header_spec_and_scheme_round_trip() {
    for (scheme, wire) in [(HeaderScheme::Raw, "raw"), (HeaderScheme::Bearer, "bearer")] {
        assert_eq!(serde_json::to_value(scheme).unwrap(), json!(wire));
        let spec = HeaderSpec {
            name: "x-api-key".into(),
            scheme,
        };
        let back: HeaderSpec = serde_json::from_value(json!({
            "name": "x-api-key",
            "scheme": wire
        }))
        .unwrap();
        assert_eq!(back, spec);
        assert_eq!(spec.clone(), spec);
        assert!(!format!("{spec:?}").is_empty());
    }
}

#[test]
fn provider_deserializes_a_full_row() {
    let p: Provider = serde_json::from_value(json!({
        "name": "anthropic",
        "base_url": "https://api.anthropic.com",
        "protocol": "anthropic_messages",
        "auth": "api_key",
        "api_header": { "name": "x-api-key", "scheme": "raw" },
        "beta_headers": [["anthropic-version", "2023-06-01"]],
        "model_aliases": { "sonnet": "claude-3-5-sonnet" }
    }))
    .unwrap();

    assert_eq!(p.name, "anthropic");
    assert_eq!(p.base_url, "https://api.anthropic.com");
    assert_eq!(p.protocol, ProtocolId::AnthropicMessages);
    assert_eq!(p.auth, AuthId::ApiKey);
    let header = p.api_header.as_ref().unwrap();
    assert_eq!(header.name, "x-api-key");
    assert_eq!(header.scheme, HeaderScheme::Raw);
    assert_eq!(
        p.beta_headers,
        vec![("anthropic-version".to_string(), "2023-06-01".to_string())]
    );
    assert_eq!(
        p.model_aliases.get("sonnet").map(String::as_str),
        Some("claude-3-5-sonnet")
    );

    assert_eq!(p.clone(), p);
    assert!(!format!("{p:?}").is_empty());
}

#[test]
fn provider_defaults_fill_the_optional_rows() {
    let p: Provider = serde_json::from_value(json!({
        "name": "openai",
        "base_url": "https://api.openai.com/v1",
        "protocol": "openai_chat",
        "auth": "bearer",
        "api_header": { "name": "Authorization", "scheme": "bearer" }
    }))
    .unwrap();
    assert!(p.beta_headers.is_empty());
    assert_eq!(p.model_aliases, BTreeMap::new());
}

#[test]
fn oauth_config_deserializes_with_defaults() {
    let cfg: OAuthConfig = serde_json::from_value(json!({
        "authorize_url": "https://auth.example/authorize",
        "token_url": "https://auth.example/token",
        "client_id": "cid",
        "beta_headers": [["anthropic-beta", "oauth-2025-04-20"]]
    }))
    .unwrap();
    assert_eq!(cfg.device_url, None);
    assert_eq!(cfg.scope, None);
    assert_eq!(cfg.client_id, "cid");
    assert_eq!(cfg.beta_headers.len(), 1);
    assert_eq!(cfg.clone(), cfg);
    assert!(!format!("{cfg:?}").is_empty());
}

#[test]
fn auth_ctx_projects_store_key_inline_key_header_and_oauth() {
    let secret = Secret::new("inline");
    let header = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let oauth = OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: None,
        client_id: "cid".into(),
        scope: Some("read".into()),
        beta_headers: vec![],
        system_preamble: None,
        redirect: brazen::RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
    };
    let ctx = AuthCtx {
        store_key: "anthropic",
        inline_key: Some(&secret),
        api_header: Some(&header),
        oauth: Some(&oauth),
        ambient: None,
    };
    assert_eq!(ctx.store_key, "anthropic");
    assert_eq!(ctx.inline_key.map(Secret::expose), Some("inline"));
    assert_eq!(ctx.api_header.map(|h| h.name.as_str()), Some("x-api-key"));
    assert_eq!(ctx.oauth.map(|o| o.client_id.as_str()), Some("cid"));
}
