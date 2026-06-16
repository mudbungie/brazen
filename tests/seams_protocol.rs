//! Seams: the `WireRequest` that flows encode → auth → transport, the framing
//! types (`Frame`/`Framing`/`DecodeState`/`OpenBlock`), the secret-free
//! `ProviderCtx`, and the no-match-on-name `Registry` (arch §4.1, §4.4, sse §3–§5).

use brazen::protocol::frame::OpenBlock;
use brazen::{
    AuthId, ContentKind, DecodeState, Frame, Framing, HeaderScheme, HeaderSpec, ProtocolId,
    ProviderCtx, Registry, Usage, WireRequest,
};
use serde_json::{Map, Value};

#[test]
fn wire_request_constructors_and_headers() {
    let mut wire = WireRequest::new("https://api.example/v1/chat", b"{}".to_vec());
    assert_eq!(wire.url, "https://api.example/v1/chat");
    assert_eq!(wire.body, b"{}");
    assert!(wire.headers.is_empty());

    // Append, then a case-insensitive overwrite (never a duplicate).
    wire.set_header("X-Api-Key", "first");
    wire.set_header("x-api-key", "second");
    assert_eq!(wire.headers.len(), 1);
    assert_eq!(wire.header("X-API-KEY"), Some("second"));
    assert_eq!(wire.header("absent"), None);

    assert_eq!(wire.clone(), wire);
    assert!(!format!("{wire:?}").is_empty());
}

#[test]
fn wire_request_raw_and_default() {
    let raw = WireRequest::raw(b"verbatim".to_vec());
    assert_eq!(raw.url, "");
    assert!(raw.headers.is_empty());
    assert_eq!(raw.body, b"verbatim");
    assert_eq!(WireRequest::default(), WireRequest::new("", Vec::new()));
}

#[test]
fn frame_into_bytes_and_as_str() {
    let frame = Frame {
        event: Some("content_block_delta".into()),
        data: b"{\"type\":\"x\"}".to_vec(),
        whole_body: false,
    };
    assert_eq!(frame.as_str().unwrap(), "{\"type\":\"x\"}");
    assert_eq!(frame.clone(), frame);
    assert!(!format!("{frame:?}").is_empty());
    assert_eq!(frame.into_bytes(), b"{\"type\":\"x\"}");

    // Invalid UTF-8 surfaces as an error from as_str, never a panic.
    let bad = Frame {
        event: None,
        data: vec![0xff, 0xfe],
        whole_body: true,
    };
    assert!(bad.as_str().is_err());
}

#[test]
fn framing_is_data() {
    for f in [Framing::Sse, Framing::Ndjson, Framing::Identity] {
        assert_eq!(f, f); // Copy + PartialEq
        assert!(!format!("{f:?}").is_empty());
    }
    assert_ne!(Framing::Sse, Framing::Identity);
}

#[test]
fn decode_state_holds_open_blocks_usage_and_terminated() {
    let mut state = DecodeState::default();
    assert!(state.open.is_empty());
    assert!(!state.terminated);
    assert_eq!(state.usage, Usage::default());

    let block = OpenBlock {
        kind: ContentKind::ToolUse {
            id: "tu_1".into(),
            name: "get_weather".into(),
        },
        buffer: String::new(),
    };
    state.open.insert(0, block.clone());
    state.usage.input = Some(12);
    state.terminated = true;

    assert_eq!(state.open.get(&0), Some(&block));
    assert_eq!(
        state.open.get(&0).map(|b| b.buffer.clone()),
        Some(String::new())
    );
    assert_eq!(state.usage.input, Some(12));
    assert!(state.terminated);
    assert!(!format!("{state:?}").is_empty());
    assert!(!format!("{block:?}").is_empty());
    assert_eq!(block.clone(), block);
}

#[test]
fn provider_ctx_is_a_secret_free_projection() {
    let api_header = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let beta: Vec<(&str, &str)> = vec![("anthropic-version", "2023-06-01")];
    let extra: Map<String, Value> = Map::new();
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-3-5-sonnet",
        api_header: &api_header,
        beta_headers: &beta,
        extra: &extra,
    };
    assert_eq!(ctx.base_url, "https://api.anthropic.com");
    assert_eq!(ctx.model, "claude-3-5-sonnet");
    assert_eq!(ctx.api_header.scheme, HeaderScheme::Raw);
    assert_eq!(ctx.beta_headers, [("anthropic-version", "2023-06-01")]);
    assert!(ctx.extra.is_empty());
}

#[test]
fn registry_builtin_dispatches_by_id() {
    let reg = Registry::builtin();
    // The two staleness-free auth impls ship; `anthropic_messages` and
    // `openai_chat` are registered by their tasks. OAuth2 fails closed until its task.
    assert!(reg.protocol(ProtocolId::OpenAiChat).is_some());
    assert!(reg.protocol(ProtocolId::AnthropicMessages).is_some());
    assert!(reg.auth(AuthId::ApiKey).is_some());
    assert!(reg.auth(AuthId::Bearer).is_some());
    assert!(reg.auth(AuthId::OAuth2).is_none());
}
