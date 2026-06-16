//! `encode` projection coverage (anthropic-messages §2): the worked example, the
//! REQUIRED-field/text-only-slot rejections, every `Content`/`tool_choice`/image
//! variant, the thinking-drop and system-hoist rules, and `extra` precedence.

use brazen::{
    CanonicalError, CanonicalRequest, ErrorKind, HeaderScheme, HeaderSpec, Protocol, ProviderCtx,
    WireRequest,
};
use serde_json::{json, Map, Value};

use brazen::protocol::anthropic::AnthropicMessages;

/// Encode `req` against a fixed Anthropic-shaped ctx (model + anthropic-version).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let api = HeaderSpec {
        name: "x-api-key".into(),
        scheme: HeaderScheme::Raw,
    };
    let beta = [("anthropic-version", "2023-06-01")];
    let extra = Map::new();
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-opus-4-8",
        api_header: &api,
        beta_headers: &beta,
        extra: &extra,
    };
    AnthropicMessages.encode(req, &ctx)
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
        "system": [{"type":"text","text":"You are a terse weather bot."}],
        "messages": [
            {"role":"user","content":[{"type":"text","text":"Weather in SF?"}]},
            {"role":"assistant","content":[
                {"type":"thinking","text":"think","signature":"EqQBsig"},
                {"type":"tool_use","id":"toolu_01A","name":"get_weather",
                 "input":{"location":"San Francisco, CA"}}
            ]},
            {"role":"tool","content":[
                {"type":"tool_result","tool_use_id":"toolu_01A",
                 "content":[{"type":"text","text":"62F, foggy"}],"is_error":false}
            ]}
        ],
        "tools": [{"name":"get_weather","description":"Look up current weather",
                   "input_schema":{"type":"object",
                     "properties":{"location":{"type":"string"}},"required":["location"]}}],
        "tool_choice": {"type":"auto"},
        "max_tokens": 1024, "temperature": 0.5, "stop": ["\n\nHuman:"], "stream": true,
        "thinking": {"type":"adaptive","display":"summarized"}
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(wire.header("content-type"), Some("application/json"));
    assert_eq!(wire.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(wire.header("x-api-key"), None); // set by Auth, never encode

    let b: Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(
        b,
        json!({
            "model": "claude-opus-4-8",
            "max_tokens": 1024,
            "system": [{"type":"text","text":"You are a terse weather bot."}],
            "messages": [
                {"role":"user","content":[{"type":"text","text":"Weather in SF?"}]},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"think","signature":"EqQBsig"},
                    {"type":"tool_use","id":"toolu_01A","name":"get_weather",
                     "input":{"location":"San Francisco, CA"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"toolu_01A",
                     "content":[{"type":"text","text":"62F, foggy"}]}
                ]}
            ],
            "tools": [{"name":"get_weather","description":"Look up current weather",
                       "input_schema":{"type":"object",
                         "properties":{"location":{"type":"string"}},"required":["location"]}}],
            "tool_choice": {"type":"auto"},
            "temperature": 0.5,
            "stop_sequences": ["\n\nHuman:"],
            "stream": true,
            "thinking": {"type":"adaptive","display":"summarized"}
        })
    );
}

#[test]
fn max_tokens_is_required_else_config_error() {
    let err = enc(&from(json!({"model":"x"}))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
}

#[test]
fn text_only_slots_reject_non_text_with_parse_input() {
    // system is text-only (§2.4)
    let e1 = enc(&from(json!({"model":"x","max_tokens":1,
        "system":[{"type":"image","source":{"kind":"url","url":"u"}}]})))
    .unwrap_err();
    assert_eq!(e1.kind, ErrorKind::ParseInput);
    assert_eq!(e1.exit_code(), 64);
    // tool_result.content is text/image-only (§2.5): a nested tool_use is rejected
    let e2 = enc(&from(json!({"model":"x","max_tokens":1,"messages":[
        {"role":"tool","content":[{"type":"tool_result","tool_use_id":"t",
            "content":[{"type":"tool_use","id":"i","name":"n","input":{}}]}]}]})))
    .unwrap_err();
    assert_eq!(e2.kind, ErrorKind::ParseInput);
}

#[test]
fn content_and_image_variants_project_to_wire_shapes() {
    let b = body(&from(json!({
        "model":"x","max_tokens":5,"top_p":0.5,
        "messages":[
            {"role":"user","content":[
                {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"AAA"}},
                {"type":"image","source":{"kind":"url","url":"http://img"}}
            ]},
            {"role":"assistant","content":[{"type":"redacted_thinking","data":"RD=="}]}
        ],
        "tools":[{"name":"t","input_schema":{"type":"object"}}],
        "tool_choice":{"type":"none"}
    })));
    assert_eq!(b["top_p"], json!(0.5));
    assert_eq!(b["stream"], json!(false)); // default false still emitted
    assert!(b.get("temperature").is_none());
    assert!(b.get("system").is_none());
    assert_eq!(
        b["messages"][0]["content"],
        json!([
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}},
            {"type":"image","source":{"type":"url","url":"http://img"}}
        ])
    );
    assert_eq!(
        b["messages"][1]["content"][0],
        json!({"type":"redacted_thinking","data":"RD=="})
    );
    assert_eq!(
        b["tools"][0],
        json!({"name":"t","input_schema":{"type":"object"}})
    ); // desc omitted
    assert_eq!(b["tool_choice"], json!({"type":"none"}));
}

#[test]
fn tool_result_is_error_true_and_image_content() {
    let b = body(&from(json!({"model":"x","max_tokens":1,"messages":[
        {"role":"tool","content":[{"type":"tool_result","tool_use_id":"t","is_error":true,
            "content":[{"type":"text","text":"e"},
                       {"type":"image","source":{"kind":"url","url":"u"}}]}]}]})));
    assert_eq!(
        b["messages"][0]["content"][0],
        json!({"type":"tool_result","tool_use_id":"t","is_error":true,
               "content":[{"type":"text","text":"e"},
                          {"type":"image","source":{"type":"url","url":"u"}}]})
    );
}

#[test]
fn signatureless_thinking_dropped_and_system_role_hoisted() {
    let b = body(&from(json!({"model":"x","max_tokens":1,"messages":[
        {"role":"system","content":[{"type":"text","text":"sys"}]},
        {"role":"assistant","content":[
            {"type":"thinking","text":"hmm","signature":null},
            {"type":"text","text":"hi"}]}]})));
    // System message never appears inline; only the assistant message remains.
    assert_eq!(b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(b["messages"][0]["role"], json!("assistant"));
    // The signature-less thinking block is dropped (CR-2); only the text survives.
    assert_eq!(
        b["messages"][0]["content"],
        json!([{"type":"text","text":"hi"}])
    );
}

#[test]
fn tool_choice_spellings_and_auto_omitted_without_tools() {
    let tc = |v: Value| {
        body(&from(
            json!({"model":"x","max_tokens":1,"tools":[{"name":"t","input_schema":{}}],
                   "tool_choice":v}),
        ))["tool_choice"]
            .clone()
    };
    assert_eq!(tc(json!({"type":"any"})), json!({"type":"any"}));
    assert_eq!(
        tc(json!({"type":"tool","name":"f"})),
        json!({"type":"tool","name":"f"})
    );
    // Auto with no tools omits the field entirely.
    let b = body(&from(json!({"model":"x","max_tokens":1})));
    assert!(b.get("tool_choice").is_none());
    assert!(b.get("tools").is_none());
}

#[test]
fn extra_merges_top_level_but_typed_fields_win() {
    let b = body(&from(json!({"model":"x","max_tokens":1,
        "stop":["X"], "stop_sequences":["Y"], "metadata":{"user_id":"u"}})));
    assert_eq!(b["stop_sequences"], json!(["X"])); // typed `stop` wins over extra
    assert_eq!(b["metadata"], json!({"user_id":"u"})); // unmodelled key passes through
}
