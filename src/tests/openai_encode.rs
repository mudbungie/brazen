//! `encode` request-shape coverage (openai-chat-mapping §2): the worked example,
//! scalars/`stream_options`, `tool_choice`, `parallel_tool_calls`, `reasoning`,
//! and `extra` precedence. The per-`Content`/message projection lives in
//! `openai_encode_content`. No network — pure `(req, ctx)` → body assertions.

use crate::protocol::openai::OpenAiChat;
use crate::{CanonicalError, CanonicalRequest, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

/// Encode `req` against a fixed OpenAI-shaped ctx (bearer header, NO beta headers).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o",
        beta_headers: &[],
        exec: None,
    };
    OpenAiChat.encode(req, &ctx)
}

fn from(v: Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}
fn body(req: &CanonicalRequest) -> Value {
    serde_json::from_slice(&enc(req).unwrap().body).unwrap()
}

#[test]
fn worked_example_projects_every_field_and_header() {
    let req = from(json!({
        "model": "ignored-encode-uses-ctx",
        "system": [{"type":"text","text":"You are concise."}],
        "messages": [
            {"role":"user","content":[
                {"type":"text","text":"What's in this image, and the weather in Paris?"},
                {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"iVBORw0KG.."}}
            ]},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"call_abc","name":"get_weather","input":{"location":"Paris"}}
            ]},
            {"role":"tool","content":[
                {"type":"tool_result","tool_use_id":"call_abc",
                 "content":[{"type":"text","text":"18C, clear"}],"is_error":false}
            ]}
        ],
        "tools": [{"name":"get_weather","description":"Current weather",
                   "input_schema":{"type":"object",
                     "properties":{"location":{"type":"string"}},"required":["location"]}}],
        "tool_choice": {"type":"auto"},
        "temperature": 0.5, "stop": [], "stream": true
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "https://api.openai.com/v1/chat/completions");
    // content-type is no longer encode's job — `serve` stamps it from the dialect's
    // one home, `Protocol::content_type()` (bl-da81), so `--raw` carries it too.
    assert_eq!(wire.header("content-type"), None);
    assert_eq!(OpenAiChat.content_type(), "application/json");
    assert_eq!(wire.header("authorization"), None); // set by Auth, never encode

    let b: Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(
        b,
        json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are concise."},
                {"role": "user", "content": [
                    {"type": "text", "text": "What's in this image, and the weather in Paris?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KG.."}}
                ]},
                {"role": "assistant", "tool_calls": [
                    {"id": "call_abc", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"location\":\"Paris\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_abc", "content": "18C, clear"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "get_weather", "description": "Current weather",
                "parameters": {"type": "object",
                    "properties": {"location": {"type": "string"}}, "required": ["location"]}}}],
            "temperature": 0.5,
            "stream": true,
            "stream_options": {"include_usage": true}
        })
    );
    // tool_choice omitted (Auto); stop omitted (empty); max_tokens omitted (None);
    // top_p omitted; assistant carries tool_calls with NO content key.
    assert!(b.get("tool_choice").is_none());
    assert!(b.get("stop").is_none());
    assert!(b.get("max_tokens").is_none());
    assert!(b.get("top_p").is_none());
    assert!(b["messages"][2].get("content").is_none());
}

#[test]
fn beta_headers_are_stamped_by_serve_not_encode() {
    // A row's beta_headers (e.g. a Mistral-style `openai-beta`) ride `ctx.beta_headers`,
    // stamped once in `serve` for BOTH the encoded and `--raw` paths (bl-3e2f) — encode
    // never folds them in, so `--raw` (which skips encode) carries them too.
    let beta = [("openai-beta", "assistants=v2")];
    let ctx = ProviderCtx {
        base_url: "https://api.mistral.ai/v1",
        model: "mistral-large",
        beta_headers: &beta,
        exec: None,
    };
    let wire = OpenAiChat
        .encode(&from(json!({"model":"x","messages":[]})), &ctx)
        .unwrap();
    assert_eq!(wire.url, "https://api.mistral.ai/v1/chat/completions");
    assert_eq!(wire.header("openai-beta"), None);
}

#[test]
fn scalars_and_stream_options_only_when_streaming() {
    let b = body(&from(json!({
        "model":"x","max_tokens":256,"temperature":0.5,"top_p":0.25,
        "stop":["STOP","\n\n"],"stream":false,
        "messages":[{"role":"user","content":"hi"}]
    })));
    assert_eq!(b["max_tokens"], json!(256));
    assert_eq!(b["temperature"], json!(0.5));
    assert_eq!(b["top_p"], json!(0.25));
    assert_eq!(b["stop"], json!(["STOP", "\n\n"])); // array form, never a bare string
    assert_eq!(b["stream"], json!(false));
    assert!(b.get("stream_options").is_none()); // only set when stream == true
    assert_eq!(b["messages"][0]["content"], json!("hi")); // single text → bare string
}

#[test]
fn tool_choice_spellings() {
    let tc = |v: Value| {
        body(&from(json!({"model":"x","messages":[],
            "tools":[{"name":"t","input_schema":{}}],"tool_choice":v})))
        .get("tool_choice")
        .cloned()
    };
    assert_eq!(tc(json!({"type":"any"})), Some(json!("required")));
    assert_eq!(tc(json!({"type":"none"})), Some(json!("none")));
    assert_eq!(
        tc(json!({"type":"tool","name":"f"})),
        Some(json!({"type": "function", "function": {"name": "f"}}))
    );
    assert_eq!(tc(json!({"type":"auto"})), None); // Auto omitted
}

#[test]
fn parallel_tool_calls_projects_top_level() {
    // The canonical knob lands as OpenAI's top-level `parallel_tool_calls` (§2.6).
    let b = body(&from(
        json!({"model":"x","messages":[],"parallel_tool_calls":false}),
    ));
    assert_eq!(b["parallel_tool_calls"], json!(false));
    // None → omitted entirely (OpenAI's own default applies).
    let b = body(&from(json!({"model":"x","messages":[]})));
    assert!(b.get("parallel_tool_calls").is_none());
}

#[test]
fn extra_merges_top_level_but_typed_fields_win() {
    let b = body(&from(json!({"model":"x","messages":[],
        "stream":true,"stream_options":{"include_usage":false},
        "seed":42,"reasoning_effort":"high"})));
    assert_eq!(b["stream_options"], json!({"include_usage": true})); // typed wins over extra
    assert_eq!(b["seed"], json!(42)); // unmodelled key passes through
    assert_eq!(b["reasoning_effort"], json!("high"));
}

#[test]
fn reasoning_omits_sampling_and_renames_max_tokens() {
    // `req.reasoning` IS the reasoning-model signal (§2.7): an o-series/gpt-5 request
    // REJECTS the deprecated `max_tokens` (wants `max_completion_tokens`) and 400s on
    // non-default temperature/top_p — the same reframe as the Anthropic rule (providers §6).
    let b = body(&from(json!({
        "model":"x","messages":[],"reasoning":"high",
        "max_tokens":256,"temperature":0.5,"top_p":0.25
    })));
    assert_eq!(b["reasoning_effort"], json!("high"));
    assert!(b.get("temperature").is_none()); // dropped with reasoning
    assert!(b.get("top_p").is_none());
    assert_eq!(b["max_completion_tokens"], json!(256)); // renamed for the reasoning model
    assert!(b.get("max_tokens").is_none()); // never the deprecated key when reasoning is set

    // Without reasoning the plain keys stand: `max_tokens` and sampling emit as-is.
    let b = body(&from(json!({
        "model":"x","messages":[],"max_tokens":256,"temperature":0.5,"top_p":0.25
    })));
    assert_eq!(b["max_tokens"], json!(256));
    assert!(b.get("max_completion_tokens").is_none());
    assert_eq!(b["temperature"], json!(0.5));
    assert_eq!(b["top_p"], json!(0.25));
}

#[test]
fn reasoning_effort_projects_the_string_and_the_typed_knob_wins() {
    // The typed canonical knob → the `reasoning_effort` string (providers §6).
    let b = body(&from(
        json!({"model":"x","messages":[],"reasoning":"medium"}),
    ));
    assert_eq!(b["reasoning_effort"], json!("medium"));
    // None omits the key.
    let b = body(&from(json!({"model":"x","messages":[]})));
    assert!(b.get("reasoning_effort").is_none());
    // Typed `reasoning` is written before the extra fold, so it WINS over a raw
    // `reasoning_effort` passthrough on the same wire key.
    let b = body(&from(json!({
        "model":"x","messages":[],"reasoning":"low","reasoning_effort":"high"
    })));
    assert_eq!(b["reasoning_effort"], json!("low"));
}

#[test]
fn structured_output_and_strict_tool_project_and_typed_wins() {
    // `output` json mode → `response_format:{type:"json_object"}` (§2.5.1).
    let b = body(&from(
        json!({"model":"x","messages":[],"output":{"type":"json"}}),
    ));
    assert_eq!(b["response_format"], json!({"type": "json_object"}));
    // json_schema → nested under `json_schema` (chat's shape); `name` defaults, `strict` folds.
    let b = body(&from(json!({"model":"x","messages":[],
        "output":{"type":"json_schema","schema":{"type":"object"},"strict":true}})));
    assert_eq!(
        b["response_format"],
        json!({"type":"json_schema","json_schema":{"name":"response","schema":{"type":"object"},"strict":true}})
    );
    // None omits; typed `output` wins over a raw `response_format` passthrough.
    let b = body(&from(json!({"model":"x","messages":[]})));
    assert!(b.get("response_format").is_none());
    let b = body(&from(json!({"model":"x","messages":[],
        "output":{"type":"json"},"response_format":{"type":"text"}})));
    assert_eq!(b["response_format"], json!({"type": "json_object"}));
    // A strict custom tool folds `strict` onto the `function` object (§2.5).
    let b = body(&from(json!({"model":"x","messages":[],
        "tools":[{"name":"f","input_schema":{"type":"object"},"strict":true}]})));
    assert_eq!(b["tools"][0]["function"]["strict"], json!(true));
}
