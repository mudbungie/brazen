//! `encode` content/message projection (openai-chat-mapping §2.3–§2.5, §2.9):
//! the thinking-drop and tool-call collection rules, tool-result and image
//! variants, system-slot handling, and the text-only-slot rejections. The
//! request-shape coverage lives in `openai_encode`. Per-file harness copy, like
//! the fixture siblings. No network — pure `(req, ctx)` → body assertions.

use crate::protocol::openai::OpenAiChat;
use crate::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
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
fn document_base64_projects_to_file_part_and_url_rejects() {
    // A base64 document → a `{type:"file"}` part: data-URI `file_data` plus a `filename`
    // synthesized from the media type (chat requires it) (§2.2, §6 CR-6).
    let b = body(&from(json!({"model":"x","messages":[
        {"role":"user","content":[
            {"type":"document","source":{"kind":"base64","media_type":"application/pdf","data":"JVBER"}}]}]})));
    assert_eq!(
        b["messages"][0]["content"],
        json!([{"type":"file","file":{
            "filename":"document.pdf","file_data":"data:application/pdf;base64,JVBER"}}])
    );
    // A document URL REJECTS — chat file inputs accept no external URL (unlike image_url).
    let e = enc(&from(json!({"model":"x","messages":[
        {"role":"user","content":[{"type":"document","source":{"kind":"url","url":"https://x/y.pdf"}}]}]})))
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::ParseInput);
    assert_eq!(e.exit_code(), 64);
    // A document in the text-only system field also rejects (§2.3).
    let e2 = enc(&from(json!({"model":"x",
        "system":[{"type":"document","source":{"kind":"base64","media_type":"application/pdf","data":"J"}}]})))
    .unwrap_err();
    assert_eq!(e2.kind, ErrorKind::ParseInput);
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
