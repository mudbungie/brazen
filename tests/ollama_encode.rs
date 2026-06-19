//! `encode` projection coverage for `ollama_chat` (providers §5.3/§5.4): the worked
//! example (system hoist, base64 images, tool calls with OBJECT arguments, params
//! nested under `options`), the text-only-slot rejections, the base64-only image
//! slot, and `extra` precedence. No network — pure `(req, ctx)` → body assertions.

use brazen::protocol::ollama_chat::OllamaChat;
use brazen::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

/// Encode `req` against a fixed Ollama-shaped ctx (bearer header + one beta header).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "http://localhost:11434",
        model: "llama3.2",
        beta_headers: &[("x-beta", "on")],
    };
    OllamaChat.encode(req, &ctx)
}

fn from(v: Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}
fn body(req: &CanonicalRequest) -> Value {
    serde_json::from_slice(&enc(req).unwrap().body).unwrap()
}
fn err(v: Value) -> CanonicalError {
    enc(&from(v)).unwrap_err()
}

#[test]
fn worked_example_projects_every_field_header_and_options_nesting() {
    let req = from(json!({
        "system": [{"type":"text","text":"Be brief."}],
        "messages": [
            {"role":"user","content":[
                {"type":"text","text":"Look:"},
                {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"AAAA"}}
            ]},
            {"role":"assistant","content":[
                {"type":"text","text":"ok"},
                {"type":"tool_use","id":"call_1","name":"get_weather","input":{"location":"Paris"}},
                {"type":"thinking","text":"hmm","signature":null}
            ]},
            {"role":"tool","content":[
                {"type":"tool_result","tool_use_id":"call_1",
                 "content":[{"type":"text","text":"18C"}],"is_error":false}
            ]}
        ],
        "tools": [
            {"name":"get_weather","description":"Current","input_schema":{"type":"object"}},
            {"name":"noop","input_schema":{"type":"object"}}
        ],
        "max_tokens": 256, "temperature": 0.5, "top_p": 0.25, "stop": ["X"],
        "stream": true, "keep_alive": "5m"
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "http://localhost:11434/api/chat");
    // content-type is no longer encode's job — `serve` stamps it from the dialect's
    // one home, `Protocol::content_type()` (bl-da81), so `--raw` carries it too.
    assert_eq!(wire.header("content-type"), None);
    assert_eq!(OllamaChat.content_type(), "application/json");
    assert_eq!(wire.header("x-beta"), Some("on")); // ctx.beta_headers ride verbatim
    assert_eq!(wire.header("authorization"), None); // set by Auth, never encode

    assert_eq!(
        body(&req),
        json!({
            "model": "llama3.2",
            "messages": [
                {"role":"system","content":"Be brief."},
                {"role":"user","content":"Look:","images":["AAAA"]},
                {"role":"assistant","content":"ok",
                 "tool_calls":[{"function":{"name":"get_weather","arguments":{"location":"Paris"}}}]},
                {"role":"tool","content":"18C","tool_name":"get_weather"}
            ],
            "tools": [
                {"type":"function","function":{"name":"get_weather","parameters":{"type":"object"},"description":"Current"}},
                {"type":"function","function":{"name":"noop","parameters":{"type":"object"}}}
            ],
            "options": {"num_predict":256,"temperature":0.5,"top_p":0.25,"stop":["X"]},
            "stream": true,
            "keep_alive": "5m"
        })
    );
}

#[test]
fn minimal_request_omits_tools_and_options() {
    let req = from(json!({
        "messages": [{"role":"system","content":"hi"}], "stream": false
    }));
    assert_eq!(
        body(&req),
        json!({
            "model": "llama3.2",
            "messages": [{"role":"system","content":"hi"}],
            "stream": false
        })
    );
}

#[test]
fn tool_result_error_flag_surfaces_textually() {
    let req = from(json!({
        "messages": [{"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"c","content":[{"type":"text","text":"boom"}],"is_error":true}
        ]}], "stream": false
    }));
    assert_eq!(body(&req)["messages"][0]["content"], json!("[error] boom"));
}

#[test]
fn tool_message_carries_resolved_tool_name_else_omits_it() {
    // resolved: the originating ToolUse in the same request supplies `tool_name`
    // (§5.4), aligning the result to its call by name as Ollama's /api/chat expects.
    let resolved = from(json!({"messages":[
        {"role":"assistant","content":[
            {"type":"tool_use","id":"call_0","name":"get_weather","input":{}}
        ]},
        {"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"call_0","content":[{"type":"text","text":"18C"}],"is_error":false}
        ]}
    ], "stream": false}));
    assert_eq!(
        body(&resolved)["messages"][1],
        json!({"role":"tool","content":"18C","tool_name":"get_weather"})
    );
    // fallback: a bare tool-result turn whose call is absent → `tool_name` omitted
    // (the name is genuinely not in-band; no fabrication).
    let bare = from(json!({"messages":[
        {"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"orphan","content":[{"type":"text","text":"18C"}],"is_error":false}
        ]}
    ], "stream": false}));
    assert_eq!(
        body(&bare)["messages"][0],
        json!({"role":"tool","content":"18C"})
    );
}

#[test]
fn url_image_is_unrepresentable_in_the_base64_only_slot() {
    let e = err(json!({
        "messages": [{"role":"user","content":[
            {"type":"image","source":{"kind":"url","url":"https://x/y.png"}}
        ]}]
    }));
    assert_eq!(e.kind, ErrorKind::ParseInput);
    assert_eq!(e.exit_code(), 64);
}

#[test]
fn text_only_slots_reject_non_text_content() {
    // a non-text system part
    assert_eq!(
        err(json!({"system":[{"type":"image","source":{"kind":"url","url":"u"}}]})).kind,
        ErrorKind::ParseInput
    );
    // an image in an assistant turn
    assert_eq!(
        err(json!({"messages":[{"role":"assistant","content":[
            {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"A"}}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
    // a tool_use part in a user turn (neither text nor image)
    assert_eq!(
        err(json!({"messages":[{"role":"user","content":[
            {"type":"tool_use","id":"c","name":"f","input":{}}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
    // a non-tool_result part in a tool turn
    assert_eq!(
        err(json!({"messages":[{"role":"tool","content":[{"type":"text","text":"x"}]}]})).kind,
        ErrorKind::ParseInput
    );
    // a non-text nested part in a tool_result
    assert_eq!(
        err(json!({"messages":[{"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"c","content":[
                {"type":"image","source":{"kind":"url","url":"u"}}],"is_error":false}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
}
