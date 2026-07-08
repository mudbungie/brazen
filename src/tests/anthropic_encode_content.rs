//! `encode` content/message projection (anthropic-messages §2.4–§2.6): every
//! `Content`/image variant, the thinking-drop and system-hoist rules, and the
//! text-only-slot rejections. The request-shape coverage lives in
//! `anthropic_encode`. Per-file harness copy, like the fixture siblings.

use crate::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

use crate::protocol::anthropic::AnthropicMessages;

/// Encode `req` against a fixed Anthropic-shaped ctx (model + anthropic-version).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let beta = [("anthropic-version", "2023-06-01")];
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-opus-4-8",
        beta_headers: &beta,
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
    // desc omitted; the auto head mark (§2.10) lands on the last tool (no system).
    assert_eq!(
        b["tools"][0],
        json!({"name":"t","input_schema":{"type":"object"},
               "cache_control":{"type":"ephemeral"}})
    );
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
    // …it HOISTS into the top-level `system` array (§2.3, architecture.md §3.1),
    // never silently dropped. (The head cache mark rides the last system block.)
    assert_eq!(b["system"][0]["type"], json!("text"));
    assert_eq!(b["system"][0]["text"], json!("sys"));
    // The signature-less thinking block is dropped (CR-2); only the text survives.
    assert_eq!(
        b["messages"][0]["content"],
        json!([{"type":"text","text":"hi"}])
    );
}

#[test]
fn system_hoist_is_req_system_first_then_role_system_in_order() {
    // `req.system` blocks lead, then each `Role::System` message's blocks in
    // transcript order — all in the ONE top-level `system` array (§2.3/§2.4). The
    // interleaved user turn is the only `messages[]` entry.
    let b = body(&from(json!({"model":"x","max_tokens":1,
        "system":[{"type":"text","text":"cfg"}],
        "messages":[
            {"role":"system","content":[{"type":"text","text":"a"}]},
            {"role":"user","content":[{"type":"text","text":"q"}]},
            {"role":"system","content":[{"type":"text","text":"b"}]}]})));
    let texts: Vec<&str> = b["system"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blk| blk["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["cfg", "a", "b"]);
    assert_eq!(b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(b["messages"][0]["role"], json!("user"));
}

#[test]
fn role_system_message_with_non_text_rejects_like_req_system() {
    // The hoisted `system` slot is text-only for BOTH sources: a non-`Text` block
    // in a `Role::System` message rejects with ParseInput/64, same as `req.system`.
    let e = enc(&from(json!({"model":"x","max_tokens":1,"messages":[
        {"role":"system","content":[
            {"type":"image","source":{"kind":"url","url":"u"}}]}]})))
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::ParseInput);
    assert_eq!(e.exit_code(), 64);
}
