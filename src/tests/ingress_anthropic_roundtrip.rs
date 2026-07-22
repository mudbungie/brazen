//! The ingress↔egress round-trip property (ingress.md §14) — the real-SDK driver for
//! `anthropic_messages`: the ACTUAL egress `AnthropicMessages` adapter encodes a canonical
//! request to the `POST /v1/messages` wire, and `decode_request` recovers it IDENTICALLY.
//! The two codecs check each other, no third source of truth. The only sanctioned deltas
//! ("modulo defaults") are the encoder's own fabrications — the automatic `cache_control`
//! marks the decoder ignores (§2.10) and the always-emitted `stream` — pinned explicitly.

use serde_json::json;

use crate::protocol::anthropic::AnthropicMessages;
use crate::{CanonicalRequest, IngressId, Protocol, ProviderCtx};

/// Encode canonically → wire body → decode through the ingress edge.
fn roundtrip(req: &CanonicalRequest) -> CanonicalRequest {
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: &req.model, // encode stamps ctx.model; equal input model → clean identity
        beta_headers: &[],
        exec: None,
    };
    let wire = AnthropicMessages.encode(req, &ctx).unwrap();
    crate::decode_request(IngressId::AnthropicMessages, &wire.body).unwrap()
}

fn req(v: serde_json::Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}

#[test]
fn worked_example_round_trips_identically() {
    // Every canonical construct this wire carries — a system prompt, media, a signed
    // thinking block before a tool call, a tool-result turn, both tool variants,
    // structured output — back through the mirror unchanged. The automatic §2.10
    // cache_control marks the encoder adds are dropped by the decoder (they have no
    // canonical home), so identity holds.
    let r = req(json!({
        "model": "claude-opus-4-8",
        "max_tokens": 1024,
        "system": [{"type": "text", "text": "You are concise."}],
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "Weather in Paris?"},
                {"type": "image", "source": {"kind": "base64", "media_type": "image/png", "data": "iVBOR=="}},
                {"type": "image", "source": {"kind": "url", "url": "https://x/a.png"}},
                {"type": "document", "source": {"kind": "url", "url": "https://x/d.pdf"}}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "text": "think", "signature": "SIG=="},
                {"type": "tool_use", "id": "toolu_a", "name": "get_weather", "input": {"location": "Paris"}}
            ]},
            {"role": "tool", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_a",
                 "content": [{"type": "text", "text": "18C"}], "is_error": false}
            ]}
        ],
        "tools": [
            {"name": "get_weather", "description": "Current", "input_schema": {"type": "object"}, "strict": true},
            {"type": "web_search_20250305", "name": "web_search", "max_uses": 3}
        ],
        "tool_choice": {"type": "auto"},
        "parallel_tool_calls": false,
        "temperature": 0.5, "top_p": 0.9,
        "stop": ["STOP"],
        "stream": false,
        "output": {"type": "json_schema", "schema": {"type": "object"}}
    }));
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn tool_choice_variants_round_trip() {
    for (tc, parallel) in [
        (json!({"type": "auto"}), json!(null)),
        (json!({"type": "any"}), json!(false)), // disable_parallel folds onto auto/any
        (json!({"type": "none"}), json!(null)),
        (json!({"type": "tool", "name": "f"}), json!(null)),
    ] {
        let r = req(json!({
            "model": "claude-x", "max_tokens": 16, "stream": true, "tool_choice": tc,
            "parallel_tool_calls": parallel,
            "tools": [{"name": "f", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        }));
        assert_eq!(roundtrip(&r), r);
    }
}

#[test]
fn server_tool_blocks_and_extra_round_trip() {
    // Server-tool blocks carried natively (never the stash), and an anthropic-specific
    // `extra` knob (top_k) folds top-level and back.
    let r = req(json!({
        "model": "claude-x", "max_tokens": 16, "stream": true, "top_k": 40,
        "messages": [{"role": "assistant", "content": [
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"q": "x"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
             "content": [{"title": "T"}]}
        ]}]
    }));
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn a_redacted_thinking_turn_round_trips() {
    let r = req(json!({
        "model": "claude-x", "max_tokens": 16, "stream": false,
        "messages": [{"role": "assistant", "content": [
            {"type": "redacted_thinking", "data": "opaque-blob"}
        ]}]
    }));
    assert_eq!(roundtrip(&r), r);
}
