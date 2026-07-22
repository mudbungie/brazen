//! The cross-dialect `Tool::Provider` degradation (providers §9, openai-chat-mapping
//! §6): provider-typed tools are carried verbatim by the Anthropic encoder ONLY —
//! every other dialect's encode fails FAST with `ParseInput` (exit 64), never a
//! silent drop, so a transcript built for one provider cannot silently lose its
//! tool declarations on another. One test, all four non-Anthropic dialects.

use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{CanonicalRequest, ErrorKind, Protocol, ProviderCtx};
use serde_json::json;

#[test]
fn provider_typed_tools_reject_with_parse_input_on_every_non_anthropic_dialect() {
    let req: CanonicalRequest = serde_json::from_value(json!({
        "model": "x", "messages": [],
        "tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}]
    }))
    .unwrap();
    let ctx = ProviderCtx {
        base_url: "https://api.example.com",
        model: "m",
        beta_headers: &[],
        exec: None,
    };
    let dialects: [&dyn Protocol; 4] = [&OpenAiChat, &OllamaChat, &OpenAiResponses, &GoogleGenAi];
    for p in dialects {
        let e = p.encode(&req, &ctx).unwrap_err();
        assert_eq!(e.kind, ErrorKind::ParseInput);
        assert_eq!(e.exit_code(), 64);
        assert!(e.message.contains("provider-typed tools"), "{}", e.message);
    }
}
