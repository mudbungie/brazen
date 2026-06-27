//! Seams: the `WireRequest` that flows encode → auth → transport, the framing
//! types (`Frame`/`Framing`/`DecodeState`/`OpenBlock`), the secret-free
//! `ProviderCtx`, and the no-match-on-name `Registry` (arch §4.1, §4.4, sse §3–§5).

use crate::protocol::frame::OpenBlock;
use crate::{
    Auth, AuthId, ContentKind, DecodeState, Frame, Framing, Method, Protocol, ProtocolId,
    ProviderCtx, Registry, WireRequest,
};

#[test]
fn wire_request_constructors_and_headers() {
    let mut wire = WireRequest::new("https://api.example/v1/chat", b"{}".to_vec());
    assert_eq!(wire.url, "https://api.example/v1/chat");
    assert_eq!(wire.body, b"{}");
    assert!(wire.headers.is_empty());
    // `new` (the encode constructor) is a POST.
    assert_eq!(wire.method, Method::Post);

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
fn wire_request_get_is_an_empty_bodied_get() {
    // The models probe / list-models constructor (§6): GET, empty body, no headers.
    let wire = WireRequest::get("https://api.example/v1/models");
    assert_eq!(wire.method, Method::Get);
    assert_eq!(wire.url, "https://api.example/v1/models");
    assert!(wire.body.is_empty());
    assert!(wire.headers.is_empty());
}

#[test]
fn method_defaults_to_post() {
    // Data on the wire (§6): the `#[default]` is Post, so `encode` (which never sets
    // it) stays a POST and the GET is the deliberate `get` opt-in.
    assert_eq!(Method::default(), Method::Post);
    assert_ne!(Method::Post, Method::Get);
    assert_eq!(Method::Get, Method::Get); // Copy + Eq
    assert!(!format!("{:?}", Method::Get).is_empty());
}

#[test]
fn wire_request_default() {
    let def = WireRequest::default();
    assert_eq!(def, WireRequest::new("", Vec::new()));
    assert_eq!(def.method, Method::Post);
}

#[test]
fn protocol_path_is_the_one_target_home() {
    // The path each protocol appends to `base_url` — the SAME string `encode` builds
    // its url from, and the seam `--raw` reuses (it skips `encode`) so a raw request
    // targets `{base_url}{path}` and is never sent to "" (bl-080b).
    let beta: Vec<(&str, &str)> = vec![];
    let ctx = ProviderCtx {
        base_url: "https://host",
        model: "M",
        beta_headers: &beta,
    };
    let reg = Registry::builtin();
    for (id, want) in [
        (ProtocolId::OpenAiChat, "/chat/completions"),
        (ProtocolId::AnthropicMessages, "/v1/messages"),
        (ProtocolId::OpenAiResponses, "/responses"),
        (ProtocolId::OllamaChat, "/api/chat"),
        // Google's path carries the model segment + the streaming verb (the --raw default).
        (
            ProtocolId::GoogleGenAi,
            "/v1beta/models/M:streamGenerateContent?alt=sse",
        ),
    ] {
        assert_eq!(reg.protocol(id).path(&ctx), want);
    }
}

#[test]
fn protocol_content_type_is_the_one_media_type_home() {
    // The dialect's body media type — DATA, like `path` — with ONE home per protocol.
    // `serve` stamps it onto BOTH the encoded and the `--raw` wire, so neither the
    // `encode`s nor the raw arm hardcodes the string (bl-da81). Every shipped dialect
    // is JSON today; a future non-JSON protocol overrides just this method.
    let reg = Registry::builtin();
    for id in [
        ProtocolId::OpenAiChat,
        ProtocolId::AnthropicMessages,
        ProtocolId::OpenAiResponses,
        ProtocolId::OllamaChat,
        ProtocolId::GoogleGenAi,
    ] {
        assert_eq!(reg.protocol(id).content_type(), "application/json");
    }
}

#[test]
fn frame_into_bytes_and_as_str() {
    let frame = Frame {
        event: Some("content_block_delta".into()),
        data: b"{\"type\":\"x\"}".to_vec(),
        status: None,
    };
    assert_eq!(frame.as_str().unwrap(), "{\"type\":\"x\"}");
    assert_eq!(frame.clone(), frame);
    assert!(!format!("{frame:?}").is_empty());
    assert_eq!(frame.into_bytes(), b"{\"type\":\"x\"}");

    // Invalid UTF-8 surfaces as an error from as_str, never a panic.
    let bad = Frame {
        event: None,
        data: vec![0xff, 0xfe],
        status: Some(500),
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
fn decode_state_holds_open_blocks_and_terminated() {
    let mut state = DecodeState::default();
    assert!(state.open.is_empty());
    assert!(!state.terminated);

    let block = OpenBlock {
        kind: ContentKind::ToolUse {
            id: "tu_1".into(),
            name: "get_weather".into(),
        },
    };
    state.open.insert(0, block.clone());
    state.terminated = true;

    assert_eq!(state.open.get(&0), Some(&block));
    assert!(state.terminated);
    assert!(!format!("{state:?}").is_empty());
    assert!(!format!("{block:?}").is_empty());
    assert_eq!(block.clone(), block);
}

#[test]
fn provider_ctx_is_a_secret_free_projection() {
    let beta: Vec<(&str, &str)> = vec![("anthropic-version", "2023-06-01")];
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-3-5-sonnet",
        beta_headers: &beta,
    };
    assert_eq!(ctx.base_url, "https://api.anthropic.com");
    assert_eq!(ctx.model, "claude-3-5-sonnet");
    assert_eq!(ctx.beta_headers, [("anthropic-version", "2023-06-01")]);
}

#[test]
fn registry_resolves_every_key() {
    // Dispatch is a total match in `registry.rs` (the compiler enforces an arm per
    // variant); this forces every arm to be EXECUTED so the 100% gate backs
    // completeness — a new variant left off this list is an uncovered arm.
    let reg = Registry::builtin();
    for id in [
        ProtocolId::OpenAiChat,
        ProtocolId::AnthropicMessages,
        ProtocolId::OpenAiResponses,
        ProtocolId::GoogleGenAi,
        ProtocolId::OllamaChat,
    ] {
        let _: &dyn Protocol = reg.protocol(id);
    }
    for id in [AuthId::ApiKey, AuthId::Bearer, AuthId::OAuth2, AuthId::None] {
        let _: &dyn Auth = reg.auth(id);
    }
}
