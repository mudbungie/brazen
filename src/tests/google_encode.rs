//! `encode` projection coverage for `google_generative_ai` (providers §4.2/§4.3):
//! the worked example (system hoist, `user`/`model` roles, structured `inlineData`
//! images, `functionCall`/`functionResponse`, params under `generationConfig`), the
//! streaming-vs-not endpoint choice, `toolConfig` modes, and the text-only-slot
//! rejections. No network — pure `(req, ctx)` → body assertions.

use crate::protocol::google_genai::GoogleGenAi;
use crate::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

/// Encode `req` against a fixed Google-shaped ctx (the `x-goog-api-key` row header).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://generativelanguage.googleapis.com",
        model: "gemini-1.5-flash",
        beta_headers: &[("x-goog-beta", "v1")],
    };
    GoogleGenAi.encode(req, &ctx)
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
fn worked_example_projects_roles_images_tools_and_system_hoist() {
    let req = from(json!({
        "system": [{"type":"text","text":"Be brief."}],
        "messages": [
            {"role":"system","content":[{"type":"text","text":" Also accurate."}]},
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
        "max_tokens": 256, "temperature": 0.5, "top_p": 0.25, "stop": ["X"],
        "stream": true, "safetySettings": [{"category":"X"}]
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(
        wire.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:streamGenerateContent?alt=sse"
    );
    // content-type is no longer encode's job — `serve` stamps it from the dialect's
    // one home, `Protocol::content_type()` (bl-da81), so `--raw` carries it too.
    assert_eq!(wire.header("content-type"), None);
    assert_eq!(GoogleGenAi.content_type(), "application/json");
    // beta_headers ride via `serve` (ctx.beta_headers) for both paths, not encode (bl-3e2f).
    assert_eq!(wire.header("x-goog-beta"), None);
    assert_eq!(wire.header("x-goog-api-key"), None); // set by Auth, never encode

    assert_eq!(
        body(&req),
        json!({
            "systemInstruction": {"parts":[{"text":"Be brief. Also accurate."}]},
            "contents": [
                {"role":"user","parts":[
                    {"text":"Look:"},
                    {"inlineData":{"mimeType":"image/png","data":"AAAA"}},
                    {"fileData":{"fileUri":"https://x/y.png"}}
                ]},
                {"role":"model","parts":[
                    {"text":"ok"},
                    {"functionCall":{"name":"get_weather","args":{"location":"Paris"}}}
                ]},
                {"role":"user","parts":[
                    // name-keyed: the synthesized `call_1` id resolves back to the
                    // originating ToolUse's function name (§4.5), not the id
                    {"functionResponse":{"name":"get_weather","response":{"result":"18C"}}}
                ]}
            ],
            "tools": [{"functionDeclarations":[
                {"name":"get_weather","parameters":{"type":"object"},"description":"Current"},
                {"name":"noop","parameters":{"type":"object"}}
            ]}],
            "toolConfig": {"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["get_weather"]}},
            "generationConfig": {"maxOutputTokens":256,"temperature":0.5,"topP":0.25,"stopSequences":["X"]},
            "safetySettings": [{"category":"X"}]
        })
    );
}

#[test]
fn non_streaming_selects_the_generate_content_endpoint_and_minimal_body() {
    let req = from(json!({
        "messages": [{"role":"user","content":"hi"}], "stream": false
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(
        wire.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent"
    );
    // no system, no tools, no toolConfig (Auto omits), no generationConfig
    assert_eq!(
        body(&req),
        json!({ "contents": [{"role":"user","parts":[{"text":"hi"}]}] })
    );
}

#[test]
fn tool_config_modes_for_any_and_none() {
    let cfg = |tc: Value| -> Value {
        let req = from(json!({"messages":[{"role":"user","content":"x"}], "tool_choice": tc}));
        body(&req)["toolConfig"]["functionCallingConfig"].clone()
    };
    assert_eq!(cfg(json!({"type":"any"})), json!({"mode":"ANY"}));
    assert_eq!(cfg(json!({"type":"none"})), json!({"mode":"NONE"}));
}

#[test]
fn is_error_tool_result_surfaces_textually() {
    let req = from(json!({"messages":[{"role":"tool","content":[
        {"type":"tool_result","tool_use_id":"c","content":[{"type":"text","text":"boom"}],"is_error":true}
    ]}]}));
    assert_eq!(
        body(&req)["contents"][0]["parts"][0]["functionResponse"]["response"]["result"],
        json!("[error] boom")
    );
}

#[test]
fn function_response_name_resolves_from_tool_use_else_falls_back_to_id() {
    // resolved: a ToolUse{id:"call_0_0", name:"get_weather"} earlier in the request
    // makes the result name-keyed to "get_weather" (§4.5), the function name Google
    // matches on — NOT the synthesized id it never sent.
    let resolved = from(json!({"messages":[
        {"role":"assistant","content":[
            {"type":"tool_use","id":"call_0_0","name":"get_weather","input":{}}
        ]},
        {"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"call_0_0","content":[{"type":"text","text":"18C"}],"is_error":false}
        ]}
    ]}));
    assert_eq!(
        body(&resolved)["contents"][1]["parts"][0]["functionResponse"]["name"],
        json!("get_weather")
    );
    // fallback: a bare tool-result turn whose originating ToolUse is absent → the
    // name is not in-band, so the id is used verbatim (no fabrication).
    let bare = from(json!({"messages":[
        {"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"orphan","content":[{"type":"text","text":"18C"}],"is_error":false}
        ]}
    ]}));
    assert_eq!(
        body(&bare)["contents"][0]["parts"][0]["functionResponse"]["name"],
        json!("orphan")
    );
}

#[test]
fn text_only_slots_reject_non_text_content() {
    // a non-text system part
    assert_eq!(
        err(json!({"system":[{"type":"image","source":{"kind":"url","url":"u"}}]})).kind,
        ErrorKind::ParseInput
    );
    // a non-text nested part in a tool_result (the functionResponse slot)
    assert_eq!(
        err(json!({"messages":[{"role":"tool","content":[
            {"type":"tool_result","tool_use_id":"c","content":[
                {"type":"image","source":{"kind":"url","url":"u"}}],"is_error":false}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
}

#[test]
fn reasoning_projects_thinking_config_under_generation_config() {
    // effort → thinkingConfig via the shared budget table (providers §6).
    let b = body(&from(json!({"model":"x","messages":[],"reasoning":"high"})));
    assert_eq!(
        b["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget":24576,"includeThoughts":true})
    );
    // None leaves generationConfig absent entirely (no other gen params here).
    let b = body(&from(json!({"model":"x","messages":[]})));
    assert!(b.get("generationConfig").is_none());
}
