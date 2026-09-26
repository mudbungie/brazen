//! The cross-dialect `Tool::Provider` projection (providers §9, openai-chat-mapping
//! §6): provider-typed tools are carried VERBATIM by the two dialects whose wire has
//! typed tools — Anthropic (client + server tools) and OpenAI Responses (native tools:
//! `image_generation`, `web_search`, …; bl-83b4). Every other dialect's encode fails
//! FAST with `ParseInput` (exit 64), never a silent drop, so a transcript built for
//! one provider cannot silently lose its tool declarations on another.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{CanonicalRequest, ErrorKind, Protocol, ProviderCtx};
use serde_json::{json, Value};

const CTX: ProviderCtx = ProviderCtx {
    base_url: "https://api.example.com",
    model: "m",
    beta_headers: &[],
    exec: None,
};

fn req(tools: Value) -> CanonicalRequest {
    serde_json::from_value(json!({"model": "x", "messages": [], "max_tokens": 8, "tools": tools}))
        .unwrap()
}

/// The encoded `tools` array, minus Anthropic's automatic `cache_control` mark (placed
/// on the last tool by the encoder's own policy, anthropic-messages §2.10 — not this
/// test's subject).
fn body_tools(p: &dyn Protocol, r: &CanonicalRequest) -> Value {
    let wire = p.encode(r, &CTX).unwrap();
    let mut tools = serde_json::from_slice::<Value>(&wire.body).unwrap()["tools"].take();
    for t in tools.as_array_mut().unwrap() {
        t.as_object_mut().unwrap().remove("cache_control");
    }
    tools
}

#[test]
fn provider_typed_tools_reject_with_parse_input_on_the_dialects_without_typed_tools() {
    let r = req(json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}]));
    let dialects: [&dyn Protocol; 3] = [&OpenAiChat, &OllamaChat, &GoogleGenAi];
    for p in dialects {
        let e = p.encode(&r, &CTX).unwrap_err();
        assert_eq!(e.kind, ErrorKind::ParseInput);
        assert_eq!(e.exit_code(), 64);
        assert!(e.message.contains("provider-typed tools"), "{}", e.message);
    }
}

#[test]
fn responses_carries_a_nameless_native_tool_verbatim_beside_a_custom_one() {
    // The OpenAI image path (providers §9 CR-Img): `image_generation` has NO name,
    // and its config keys ride flat. A custom tool beside it keeps its flat function shape.
    let r = req(json!([
        {"type": "image_generation", "size": "1024x1024", "partial_images": 1},
        {"name": "f", "input_schema": {"type": "object"}}
    ]));
    assert_eq!(
        body_tools(&OpenAiResponses, &r),
        json!([
            {"type": "image_generation", "size": "1024x1024", "partial_images": 1},
            {"type": "function", "name": "f", "parameters": {"type": "object"}}
        ])
    );
}

#[test]
fn anthropic_writes_the_name_only_when_the_tool_has_one() {
    // Unchanged for every existing input (a named server tool), and a nameless one is
    // carried as given — the provider, not brazen, is the authority on whether it needs one.
    let named = req(json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}]));
    assert_eq!(
        body_tools(&AnthropicMessages, &named),
        json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}])
    );
    let nameless = req(json!([{"type": "bash_20250124"}]));
    assert_eq!(
        body_tools(&AnthropicMessages, &nameless),
        json!([{"type": "bash_20250124"}])
    );
}
