//! `encode` projection coverage for `openai_responses` (providers §3.2/§3.3): the
//! worked example (`system`→`instructions`, `messages`→typed `input[]`,
//! `max_tokens`→`max_output_tokens`, FLAT tools, function_call/output items), the
//! tool_choice spellings, and the text-only-slot rejections. No network — pure
//! `(req, ctx)` → body assertions.

use brazen::protocol::openai_responses::OpenAiResponses;
use brazen::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-2024-08-06",
        beta_headers: &[("x-beta", "on")],
    };
    OpenAiResponses.encode(req, &ctx)
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
fn worked_example_folds_system_messages_and_tools_into_typed_input() {
    let req = from(json!({
        "system": [{"type":"text","text":"Be brief."}],
        "messages": [
            {"role":"system","content":[{"type":"text","text":"Sys."}]},
            {"role":"user","content":[
                {"type":"text","text":"Look:"},
                {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"AAAA"}},
                {"type":"image","source":{"kind":"url","url":"https://x/y.png"}}
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
        "tool_choice": {"type":"tool","name":"get_weather"},
        "max_tokens": 256, "temperature": 0.5, "top_p": 0.25, "stream": true,
        "reasoning": {"effort":"low"}
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "https://api.openai.com/v1/responses");
    assert_eq!(wire.header("x-beta"), Some("on"));
    assert_eq!(wire.header("authorization"), None); // set by Auth, never encode

    assert_eq!(
        body(&req),
        json!({
            "model": "gpt-4o-2024-08-06",
            "instructions": "Be brief.", // req.system hoists; Role::System stays in input[] (§3.3)
            "input": [
                {"type":"message","role":"system","content":[{"type":"input_text","text":"Sys."}]},
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"Look:"},
                    {"type":"input_image","image_url":"data:image/png;base64,AAAA"},
                    {"type":"input_image","image_url":"https://x/y.png"}
                ]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]},
                {"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"Paris\"}"},
                {"type":"function_call_output","call_id":"call_1","output":"18C"}
            ],
            "tools": [
                {"type":"function","name":"get_weather","parameters":{"type":"object"},"description":"Current"},
                {"type":"function","name":"noop","parameters":{"type":"object"}}
            ],
            "tool_choice": {"type":"function","name":"get_weather"},
            "max_output_tokens": 256, // RENAME (§3.2)
            "temperature": 0.5,
            "top_p": 0.25,
            "stream": true,
            "reasoning": {"effort":"low"}
        })
    );
}

#[test]
fn minimal_request_omits_instructions_tools_and_tool_choice() {
    let req = from(json!({
        "messages": [{"role":"user","content":"hi"}], "stream": false
    }));
    assert_eq!(
        body(&req),
        json!({
            "model": "gpt-4o-2024-08-06",
            "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}],
            "stream": false
        })
    );
}

#[test]
fn tool_choice_spellings_for_any_and_none() {
    let tc = |v: Value| -> Value {
        let req = from(json!({"messages":[{"role":"user","content":"x"}], "tool_choice": v}));
        body(&req)["tool_choice"].clone()
    };
    assert_eq!(tc(json!({"type":"any"})), json!("required"));
    assert_eq!(tc(json!({"type":"none"})), json!("none"));
}

#[test]
fn is_error_tool_result_surfaces_textually() {
    let req = from(json!({"messages":[{"role":"tool","content":[
        {"type":"tool_result","tool_use_id":"c","content":[{"type":"text","text":"boom"}],"is_error":true}
    ]}]}));
    assert_eq!(body(&req)["input"][0]["output"], json!("[error] boom"));
}

#[test]
fn text_only_slots_and_role_slots_reject_unrepresentable_content() {
    // a non-text instructions (system) part
    assert_eq!(
        err(json!({"system":[{"type":"image","source":{"kind":"url","url":"u"}}]})).kind,
        ErrorKind::ParseInput
    );
    // an image in an assistant turn (image allowed only in a user slot)
    assert_eq!(
        err(json!({"messages":[{"role":"assistant","content":[
            {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"A"}}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
    // a non-tool_result part in a tool turn
    assert_eq!(
        err(json!({"messages":[{"role":"tool","content":[{"type":"text","text":"x"}]}]})).kind,
        ErrorKind::ParseInput
    );
    // a non-text nested part in a function_call_output
    assert_eq!(
        err(json!({"messages":[{"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"c","content":[
                {"type":"image","source":{"kind":"url","url":"u"}}],"is_error":false}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
}
