//! Automatic prompt-cache placement coverage (anthropic-messages §2.10): the
//! head mark (system-last, else tools-last, else nowhere), the rolling mark on
//! an ongoing conversation (trailing-assistant prefill never triggers), the
//! eligibility walk-back (thinking tails, 0-eligible messages), the intermediate
//! mark on a long span, TTL never emitted, ≤4 marks always, and the policy
//! marker landing BEFORE the `extra` fold.

use crate::{CanonicalRequest, Protocol, ProviderCtx};
use serde_json::{json, Value};

use crate::protocol::anthropic::AnthropicMessages;

fn from(v: Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}

/// Encode against a fixed Anthropic-shaped ctx and parse the wire body back.
fn body(req: &CanonicalRequest) -> Value {
    let beta = [("anthropic-version", "2023-06-01")];
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-opus-4-8",
        beta_headers: &beta,
    };
    let wire = AnthropicMessages.encode(req, &ctx).unwrap();
    serde_json::from_slice(&wire.body).unwrap()
}

/// The one marker the policy ever writes — TTL always omitted (§2.10).
fn mark() -> Value {
    json!({"type": "ephemeral"})
}

/// Total `cache_control` occurrences anywhere in the body (the ≤4 lens).
fn mark_count(b: &Value) -> usize {
    serde_json::to_string(b)
        .unwrap()
        .matches("cache_control")
        .count()
}

#[test]
fn one_shot_head_mark_on_system_last_else_tools_last_else_nowhere() {
    // system present (two blocks): the LAST system block carries the ONE mark —
    // it caches the tools+system prefix, so tools are not marked separately.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"a"},{"type":"text","text":"b"}],
        "tools":[{"name":"t","input_schema":{}}],
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
    })));
    assert!(b["system"][0].get("cache_control").is_none());
    assert_eq!(b["system"][1]["cache_control"], mark());
    assert!(b["tools"][0].get("cache_control").is_none());
    assert_eq!(mark_count(&b), 1); // one-shot: no rolling, no intermediate

    // system empty (`Some(vec![])` — same as absent): falls to the LAST tool.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,"system":[],
        "tools":[{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}],
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
    })));
    assert!(b["tools"][0].get("cache_control").is_none());
    assert_eq!(b["tools"][1]["cache_control"], mark());
    assert_eq!(mark_count(&b), 1);

    // neither system nor tools: no mark anywhere — the body is policy-untouched.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
    })));
    assert_eq!(mark_count(&b), 0);
}

#[test]
fn ongoing_conversation_rolls_a_mark_through_the_system_hoist() {
    // [system, user, assistant, user] → wire [user, assistant, user]: the System
    // message never reaches the wire, so placement never sees it (the hoist skip
    // is inherent — marks are computed on the wire array itself).
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages":[
            {"role":"system","content":[{"type":"text","text":"sys"}]},
            {"role":"user","content":[{"type":"text","text":"q1"}]},
            {"role":"assistant","content":[{"type":"text","text":"a1"}]},
            {"role":"user","content":[
                {"type":"text","text":"q2"},{"type":"text","text":"more"}]}
        ]
    })));
    assert_eq!(b["messages"].as_array().unwrap().len(), 3); // hoisted out
    assert_eq!(b["system"][0]["cache_control"], mark()); // head
                                                         // rolling: the LAST block of the last non-assistant wire message.
    assert_eq!(b["messages"][2]["content"][1]["cache_control"], mark());
    assert!(b["messages"][2]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(mark_count(&b), 2); // short span: no intermediate
}

#[test]
fn trailing_assistant_prefill_never_anchors_a_rolling_mark() {
    // A lone trailing assistant is a prefill one-shot: the completion extends its
    // blocks, so nothing in `messages` is marked — head only.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages":[
            {"role":"user","content":[{"type":"text","text":"q"}]},
            {"role":"assistant","content":[{"type":"text","text":"The answer is"}]}
        ]
    })));
    assert_eq!(b["system"][0]["cache_control"], mark());
    assert_eq!(mark_count(&b), 1);

    // All-assistant history (no non-assistant message to anchor): head only.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages":[
            {"role":"assistant","content":[{"type":"text","text":"a"}]},
            {"role":"assistant","content":[{"type":"text","text":"b"}]}
        ]
    })));
    assert_eq!(mark_count(&b), 1);
}

#[test]
fn ineligible_tails_step_back_a_block_or_a_whole_message() {
    // The rolling target's tail is a redacted_thinking block (cache_control is
    // invalid there): the mark steps back to the previous eligible block.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "messages":[
            {"role":"user","content":[{"type":"text","text":"q"}]},
            {"role":"assistant","content":[{"type":"text","text":"a"}]},
            {"role":"user","content":[
                {"type":"text","text":"follow-up"},
                {"type":"redacted_thinking","data":"RD=="}]}
        ]
    })));
    assert_eq!(b["messages"][2]["content"][0]["cache_control"], mark());
    assert!(b["messages"][2]["content"][1]
        .get("cache_control")
        .is_none());

    // The last non-assistant message has 0 eligible blocks: walk back a whole
    // message — into stable assistant history, past its own thinking tail.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "messages":[
            {"role":"user","content":[{"type":"text","text":"q"}]},
            {"role":"assistant","content":[
                {"type":"text","text":"a"},
                {"type":"thinking","text":"t","signature":"sig"}]},
            {"role":"user","content":[{"type":"redacted_thinking","data":"RD=="}]}
        ]
    })));
    assert_eq!(b["messages"][1]["content"][0]["cache_control"], mark());
    assert_eq!(mark_count(&b), 1); // no system/tools → no head mark either

    // Nothing eligible anywhere: the walk-back exhausts — no mark at all.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "messages":[
            {"role":"user","content":[{"type":"redacted_thinking","data":"a"}]},
            {"role":"assistant","content":[{"type":"redacted_thinking","data":"b"}]},
            {"role":"user","content":[{"type":"redacted_thinking","data":"c"}]}
        ]
    })));
    assert_eq!(mark_count(&b), 0);
}

#[test]
fn long_span_adds_one_intermediate_mark_and_ttl_is_never_emitted() {
    // 11 alternating messages x 2 text blocks = 22 eligible blocks; the rolling
    // mark is the 22nd, so 21 precede it (>= the 20-block lookback) and ONE
    // intermediate mark lands exactly 20 eligible blocks before the rolling one:
    // message 0, block 1.
    let msgs: Vec<Value> = (0..11)
        .map(|i| {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            json!({"role": role, "content": [
                {"type":"text","text":format!("{i}.0")},
                {"type":"text","text":format!("{i}.1")}]})
        })
        .collect();
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages": msgs
    })));
    assert_eq!(b["messages"][0]["content"][1]["cache_control"], mark());
    assert!(b["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(b["messages"][10]["content"][1]["cache_control"], mark());
    assert_eq!(mark_count(&b), 3); // head + intermediate + rolling — always <= 4
    assert!(!serde_json::to_string(&b).unwrap().contains("ttl"));
}

#[test]
fn policy_marks_land_before_the_extra_fold_and_win() {
    // A raw `extra` key named like a typed field cannot clobber the marked typed
    // array: `apply` runs first and the fold is entry().or_insert (§2.1.1).
    let mut req = from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
    }));
    req.extra.insert(
        "system".into(),
        json!([{"type":"text","text":"raw",
                "cache_control":{"type":"ephemeral","ttl":"1h"}}]),
    );
    let b = body(&req);
    assert_eq!(
        b["system"],
        json!([{"type":"text","text":"s","cache_control":{"type":"ephemeral"}}])
    );
}
