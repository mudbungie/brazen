//! `encode` projection coverage (openai-chat-mapping §2): the worked example, the
//! text-only-slot rejections, every `Content`/`tool_choice`/image variant, the
//! thinking-drop and tool-call collection rules, `stream_options`, and `extra`
//! precedence. No network — pure `(req, ctx)` → body assertions.

use brazen::protocol::openai::OpenAiChat;
use brazen::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

/// Encode `req` against a fixed OpenAI-shaped ctx (bearer header, NO beta headers).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o",
        beta_headers: &[],
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
    assert_eq!(wire.header("content-type"), Some("application/json"));
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
fn beta_headers_ride_ctx_verbatim() {
    let beta = [("openai-beta", "assistants=v2")];
    let ctx = ProviderCtx {
        base_url: "https://api.mistral.ai/v1",
        model: "mistral-large",
        beta_headers: &beta,
    };
    let wire = OpenAiChat
        .encode(&from(json!({"model":"x","messages":[]})), &ctx)
        .unwrap();
    assert_eq!(wire.url, "https://api.mistral.ai/v1/chat/completions");
    assert_eq!(wire.header("openai-beta"), Some("assistants=v2"));
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
fn assistant_text_and_tool_calls_and_thinking_dropped() {
    // Assistant mixing thinking (dropped), text, and a tool_use.
    let b = body(&from(json!({"model":"x","messages":[
    {"role":"assistant","content":[
        {"type":"thinking","text":"hmm","signature":"sig"},
        {"type":"redacted_thinking","data":"RD=="},
        {"type":"text","text":"Let me check."},
        {"type":"tool_use","id":"c1","name":"f","input":{"a":1}}
    ]}]})));
    let m = &b["messages"][0];
    assert_eq!(m["role"], json!("assistant"));
    assert_eq!(m["content"], json!("Let me check.")); // thinking blocks dropped (§2.9)
    assert_eq!(
        m["tool_calls"],
        json!([{"id":"c1","type":"function","function":{"name":"f","arguments":"{\"a\":1}"}}])
    );
}

#[test]
fn empty_assistant_turn_emits_empty_string_content() {
    // No text, no tool calls → content is "" (not omitted), tool_calls absent.
    let b = body(&from(json!({"model":"x","messages":[
        {"role":"assistant","content":[{"type":"thinking","text":"t","signature":null}]}]})));
    assert_eq!(b["messages"][0]["content"], json!(""));
    assert!(b["messages"][0].get("tool_calls").is_none());
}

#[test]
fn multi_part_assistant_text_uses_array_form() {
    let b = body(&from(json!({"model":"x","messages":[
        {"role":"assistant","content":[
            {"type":"text","text":"a"},{"type":"text","text":"b"}]}]})));
    assert_eq!(
        b["messages"][0]["content"],
        json!([{"type":"text","text":"a"},{"type":"text","text":"b"}])
    );
}

#[test]
fn tool_result_error_prefix_and_image_url_variants() {
    let b = body(&from(json!({"model":"x","messages":[
        {"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"c1","is_error":true,
             "content":[{"type":"text","text":"boom"}]},
            {"type":"tool_result","tool_use_id":"c2","is_error":false,
             "content":[{"type":"text","text":"ok"}]}
        ]},
        {"role":"user","content":[{"type":"image","source":{"kind":"url","url":"http://img"}}]}
    ]})));
    // Each ToolResult → its own role:"tool" message; is_error surfaces textually.
    assert_eq!(
        b["messages"][0],
        json!({"role":"tool","tool_call_id":"c1","content":"[error] boom"})
    );
    assert_eq!(
        b["messages"][1],
        json!({"role":"tool","tool_call_id":"c2","content":"ok"})
    );
    assert_eq!(
        b["messages"][2]["content"],
        json!([{"type":"image_url","image_url":{"url":"http://img"}}])
    );
}

#[test]
fn system_field_and_in_band_system_message_both_emitted() {
    let b = body(&from(json!({
        "model":"x","system":[{"type":"text","text":"field-sys"}],
        "messages":[{"role":"system","content":"inband-sys"}]
    })));
    assert_eq!(
        b["messages"][0],
        json!({"role":"system","content":"field-sys"})
    );
    assert_eq!(
        b["messages"][1],
        json!({"role":"system","content":"inband-sys"})
    );
}

#[test]
fn empty_system_field_emits_no_system_message() {
    let b = body(&from(json!({"model":"x","system":[],
        "messages":[{"role":"user","content":"hi"}]})));
    assert_eq!(b["messages"][0]["role"], json!("user")); // no leading system
}

#[test]
fn text_only_slots_reject_non_text_with_parse_input() {
    // system field is text-only (§2.3): an image rejects.
    let e1 = enc(&from(json!({"model":"x",
        "system":[{"type":"image","source":{"kind":"url","url":"u"}}]})))
    .unwrap_err();
    assert_eq!(e1.kind, ErrorKind::ParseInput);
    assert_eq!(e1.exit_code(), 64);
    // tool message content is text-only (§2.4): a nested image rejects.
    let e2 = enc(&from(json!({"model":"x","messages":[
        {"role":"tool","content":[{"type":"tool_result","tool_use_id":"t",
            "content":[{"type":"image","source":{"kind":"url","url":"u"}}]}]}]})))
    .unwrap_err();
    assert_eq!(e2.kind, ErrorKind::ParseInput);
    // a non-ToolResult part in a Role::Tool message rejects.
    let e3 = enc(&from(json!({"model":"x","messages":[
        {"role":"tool","content":[{"type":"text","text":"stray"}]}]})))
    .unwrap_err();
    assert_eq!(e3.kind, ErrorKind::ParseInput);
    // a non-text part in a system message rejects.
    let e4 = enc(&from(json!({"model":"x","messages":[
        {"role":"system","content":[{"type":"image","source":{"kind":"url","url":"u"}}]}]})))
    .unwrap_err();
    assert_eq!(e4.kind, ErrorKind::ParseInput);
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
