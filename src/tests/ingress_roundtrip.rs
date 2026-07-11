//! The ingress↔egress round-trip property (ingress.md §14): for egress-golden
//! canonical requests, `decode_request(encode(req))` is IDENTITY on the canonical
//! request — the two codecs check each other, no third source of truth. The only
//! sanctioned deltas are the encoder's own fabrications ("modulo defaults"):
//! `stream_options.include_usage` injected when streaming (lands in decoded
//! `extra`), the `json_schema.name` default `"response"`, and the reasoning-mode
//! sampling drop (openai-chat-mapping §2.7/§2.8) — each pinned here explicitly.

use serde_json::json;

use crate::protocol::openai::OpenAiChat;
use crate::{CanonicalRequest, IngressId, OutputFormat, Protocol, ProviderCtx};

/// Encode canonically → wire body → decode through the ingress edge.
fn roundtrip(req: &CanonicalRequest) -> CanonicalRequest {
    let ctx = ProviderCtx {
        base_url: "https://api.openai.com/v1",
        model: &req.model, // encode stamps ctx.model; equal input model → clean identity
        beta_headers: &[],
    };
    let wire = OpenAiChat.encode(req, &ctx).unwrap();
    crate::decode_request(IngressId::OpenAiChat, &wire.body).unwrap()
}

fn req(v: serde_json::Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}

#[test]
fn worked_example_round_trips_identically() {
    // The §2.11 worked example (the egress golden of openai_encode.rs), plus an
    // is_error tool result and a document — every canonical construct this wire
    // can carry, back through the mirror unchanged.
    let r = req(json!({
        "model": "gpt-4o",
        "system": [{"type":"text","text":"You are concise."}],
        "messages": [
            {"role":"user","content":[
                {"type":"text","text":"What's in this image, and the weather in Paris?"},
                {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"iVBORw0KG.."}},
                {"type":"image","source":{"kind":"url","url":"https://example.com/a.png"}},
                {"type":"document","source":{"kind":"base64","media_type":"application/pdf","data":"JVBERi0="}}
            ]},
            {"role":"assistant","content":[
                {"type":"text","text":"Checking."},
                {"type":"tool_use","id":"call_abc","name":"get_weather","input":{"location":"Paris"}}
            ]},
            {"role":"tool","content":[
                {"type":"tool_result","tool_use_id":"call_abc",
                 "content":[{"type":"text","text":"18C, clear"}],"is_error":false},
                {"type":"tool_result","tool_use_id":"call_def",
                 "content":[{"type":"text","text":"station offline"}],"is_error":true}
            ]}
        ],
        "tools": [{"name":"get_weather","description":"Current weather",
                   "input_schema":{"type":"object",
                     "properties":{"location":{"type":"string"}},"required":["location"]},
                   "strict":true}],
        "temperature": 0.5, "top_p": 0.9, "max_tokens": 512,
        "stop": ["STOP"], "stream": false
    }));
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn reasoning_request_round_trips_through_the_renamed_key() {
    // encode emits max_completion_tokens + reasoning_effort (§2.7); decode folds
    // both back onto the ONE canonical fact each.
    let r = req(json!({
        "model": "o4-mini", "reasoning": "high", "max_tokens": 256, "stream": false,
        "messages": [{"role":"user","content":[{"type":"text","text":"hi"}]}]
    }));
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn tool_choice_variants_round_trip() {
    for tc in [
        json!({"type":"auto"}),
        json!({"type":"any"}),
        json!({"type":"none"}),
        json!({"type":"tool","name":"f"}),
    ] {
        let r = req(json!({
            "model": "gpt-4o", "stream": false, "tool_choice": tc,
            "tools": [{"name":"f","input_schema":{"type":"object"}}],
            "messages": [{"role":"user","content":[{"type":"text","text":"hi"}]}]
        }));
        assert_eq!(roundtrip(&r), r);
    }
}

#[test]
fn structured_output_and_extra_round_trip() {
    let r = req(json!({
        "model": "gpt-4o", "stream": false, "seed": 42,
        "output": {"type":"json_schema","name":"out","schema":{"type":"object"},"strict":true},
        "messages": [{"role":"user","content":[{"type":"text","text":"hi"}]},
                     {"role":"assistant","content":[]}]
    }));
    // the empty assistant turn crosses as content:"" and inverts back to []
    assert_eq!(roundtrip(&r), r);
    let r = req(json!({"model": "gpt-4o", "stream": false, "messages": [],
                       "output": {"type":"json"}}));
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn streaming_delta_is_exactly_the_encoders_fabrications() {
    // stream:true makes encode inject stream_options (§2.8) — the ONE sanctioned
    // round-trip delta, landing in decoded extra. A None json_schema name crosses
    // as the encoder's required "response" default (§2.5.1) — the other one.
    let r = req(json!({
        "model": "gpt-4o", "stream": true,
        "output": {"type":"json_schema","schema":{"type":"object"}},
        "messages": [{"role":"user","content":[{"type":"text","text":"hi"}]}]
    }));
    let mut want = r.clone();
    want.extra
        .insert("stream_options".into(), json!({"include_usage": true}));
    want.output = Some(OutputFormat::JsonSchema {
        name: Some("response".into()), // chat REQUIRES a name; the default crosses back
        schema: json!({"type": "object"}),
        strict: None,
    });
    assert_eq!(roundtrip(&r), want);
}
