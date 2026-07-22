//! `encode` projection coverage for `openai_responses` (providers §3.2/§3.3): the
//! worked example (`system`→`instructions`, `messages`→typed `input[]`,
//! `max_tokens`→`max_output_tokens`, FLAT tools, function_call/output items), the
//! tool_choice spellings, and the text-only-slot rejections. No network — pure
//! `(req, ctx)` → body assertions.

use crate::protocol::openai_responses::OpenAiResponses;
use crate::{
    CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, ReasoningEffort,
    WireRequest,
};
use serde_json::{json, Value};

fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-2024-08-06",
        beta_headers: &[("x-beta", "on")],
        exec: None,
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
        "max_tokens": 256, "temperature": 0.5, "top_p": 0.25, "stream": true
        // reasoning is exercised in its own test (it OMITS temperature/top_p, §3.2)
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "https://api.openai.com/v1/responses");
    // content-type is no longer encode's job — `serve` stamps it from the dialect's
    // one home, `Protocol::content_type()` (bl-da81), so `--raw` carries it too.
    assert_eq!(wire.header("content-type"), None);
    assert_eq!(OpenAiResponses.content_type(), "application/json");
    // beta_headers ride via `serve` (ctx.beta_headers) for both paths, not encode (bl-3e2f).
    assert_eq!(wire.header("x-beta"), None);
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
            "stream": true
        })
    );
}

#[test]
fn reasoning_omits_sampling_and_parallel_tool_calls_projects_top_level() {
    // reasoning set → temperature/top_p OMITTED (o-series/gpt-5 400 on non-default
    // sampling; §3.2, providers §6 — mirrors the Anthropic rule). parallel_tool_calls
    // rides TOP-LEVEL, as Chat Completions (openai-chat-mapping.md §2.6).
    let b = body(&from(json!({
        "model":"x","messages":[],"reasoning":"high","temperature":0.5,"top_p":0.25,
        "parallel_tool_calls":false
    })));
    assert_eq!(b["reasoning"], json!({"effort":"high"}));
    // reasoning set → also request the encrypted reasoning blob back for stateless
    // replay (bl-61a9, §3.2), the harness round-trip enabler.
    assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));
    assert!(b.get("temperature").is_none());
    assert!(b.get("top_p").is_none());
    assert_eq!(b["parallel_tool_calls"], json!(false));

    // No reasoning → sampling emitted; no `include`; parallel_tool_calls None → omitted.
    let b = body(&from(json!({
        "model":"x","messages":[],"temperature":0.5,"top_p":0.25
    })));
    assert_eq!(b["temperature"], json!(0.5));
    assert_eq!(b["top_p"], json!(0.25));
    assert!(b.get("include").is_none());
    assert!(b.get("parallel_tool_calls").is_none());
}

#[test]
fn reasoning_omitted_when_unset_and_typed_knob_wins_over_an_extra_object() {
    // None → no `reasoning` key (the empty-set path).
    let b = body(&from(json!({"model":"x","messages":[],"store":false})));
    assert!(b.get("reasoning").is_none());
    assert_eq!(b["store"], json!(false)); // a non-gen `extra` key rides to the wire (§3.2)

    // On openai_responses the canonical key and the wire key are BOTH `reasoning`, so
    // the body_defaults escape hatch (a raw object) can only reach `extra` via the row
    // (not a piped request — the typed field intercepts the key). Simulate that seam:
    // the typed `--reasoning` knob, written before the extra fold, WINS (providers §6).
    let mut req = from(json!({"model":"x","messages":[]}));
    req.reasoning = Some(ReasoningEffort::High);
    req.extra
        .insert("reasoning".into(), json!({"effort": "low"}));
    let wire = enc(&req).unwrap();
    let b: Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(b["reasoning"], json!({"effort": "high"})); // typed wins over the extra object
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

#[test]
fn document_base64_and_url_both_project_to_input_file() {
    // Responses fetches web URLs, so BOTH document sources express (§3.3, §6 CR-6):
    // base64 → `input_file` with a data-URI `file_data` + synthesized `filename`; a URL
    // → `input_file` with `file_url`.
    let b = body(&from(json!({"model":"x","messages":[
    {"role":"user","content":[
        {"type":"document","source":{"kind":"base64","media_type":"application/pdf","data":"JVBER"}},
        {"type":"document","source":{"kind":"url","url":"https://x/y.pdf"}}
    ]}]})));
    assert_eq!(
        b["input"][0]["content"],
        json!([
            {"type":"input_file","filename":"document.pdf","file_data":"data:application/pdf;base64,JVBER"},
            {"type":"input_file","file_url":"https://x/y.pdf"}
        ])
    );
    // A document outside a user slot rejects (input-only, user turns).
    assert_eq!(
        err(json!({"messages":[{"role":"assistant","content":[
            {"type":"document","source":{"kind":"base64","media_type":"application/pdf","data":"J"}}
        ]}]}))
        .kind,
        ErrorKind::ParseInput
    );
}

#[test]
fn reasoning_items_replay_only_with_encrypted_content_and_empty_summary_when_no_text() {
    // Assistant turn with three thinking blocks (bl-61a9, §3.3): one with an EMPTY
    // summary (text ""), one with a full summary + id, and one with NO
    // encrypted_content (dropped — a bare summary can't replay statelessly). Reasoning
    // items precede the message/function_call of the turn.
    let input = body(&from(json!({"messages":[{"role":"assistant","content":[
        {"type":"thinking","text":"","signature":null,"encrypted_content":"E0=="},
        {"type":"thinking","text":"why","signature":null,"id":"rs_9","encrypted_content":"E1=="},
        {"type":"thinking","text":"lost","signature":null},
        {"type":"text","text":"answer"}
    ]}]})))["input"]
        .clone();
    assert_eq!(
        input,
        json!([
            // empty text → summary [] (the empty-summary branch), no id
            {"type":"reasoning","summary":[],"encrypted_content":"E0=="},
            {"type":"reasoning","id":"rs_9",
             "summary":[{"type":"summary_text","text":"why"}],"encrypted_content":"E1=="},
            // the third thinking (no encrypted_content) is dropped — absent here
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}
        ])
    );
}

#[test]
fn structured_output_uses_flat_text_format_and_strict_tool() {
    // `output` json mode → `text.format:{type:"json_object"}` (§3.2).
    let b = body(&from(
        json!({"model":"x","messages":[],"output":{"type":"json"}}),
    ));
    assert_eq!(b["text"], json!({"format": {"type": "json_object"}}));
    // json_schema lays {type,name,schema,strict} FLAT under text.format (no json_schema wrap).
    let b = body(&from(json!({"model":"x","messages":[],
        "output":{"type":"json_schema","name":"Out","schema":{"type":"object"},"strict":true}})));
    assert_eq!(
        b["text"],
        json!({"format":{"type":"json_schema","name":"Out","schema":{"type":"object"},"strict":true}})
    );
    // None omits; typed `output` wins over a raw `text` passthrough.
    assert!(body(&from(json!({"model":"x","messages":[]})))
        .get("text")
        .is_none());
    let b = body(&from(json!({"model":"x","messages":[],
        "output":{"type":"json"},"text":{"format":{"type":"text"}}})));
    assert_eq!(b["text"], json!({"format": {"type": "json_object"}}));
    // A strict custom tool folds `strict` FLAT onto the tool (§3.2).
    let b = body(&from(json!({"model":"x","messages":[],
        "tools":[{"name":"f","input_schema":{"type":"object"},"strict":true}]})));
    assert_eq!(b["tools"][0]["strict"], json!(true));
}
