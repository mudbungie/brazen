//! Prompt-cache projection coverage (anthropic-messages §2.10): `req.cache` →
//! per-block `cache_control` on the LAST wire block of each anchored region, the
//! System-hoist skip mapping a canonical message index to its wire position, the
//! ttl spellings (FiveMin omits / OneHour emits `"1h"`), and every resolve-or-
//! ParseInput failure mode (≤4, empty tools/system, out-of-range/System/0-block).

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
fn empty_cache_leaves_body_byte_identical() {
    // The general path with empty input: `is_empty` early-returns, no marker anywhere.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],
        "tools":[{"name":"t","input_schema":{}}]
    })));
    assert!(b["tools"][0].get("cache_control").is_none());
    assert!(b["system"][0].get("cache_control").is_none());
    assert!(b["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());
}

#[test]
fn tools_anchor_five_min_marks_only_the_last_tool() {
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "tools":[{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}],
        "cache":[{"anchor":"tools"}]
    })));
    assert_eq!(b["tools"][1]["cache_control"], json!({"type":"ephemeral"}));
    assert!(b["tools"][0].get("cache_control").is_none()); // earlier tool unmarked
}

#[test]
fn system_anchor_marks_the_last_system_block() {
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "system":[{"type":"text","text":"a"},{"type":"text","text":"b"}],
        "cache":[{"anchor":"system"}]
    })));
    assert_eq!(b["system"][1]["cache_control"], json!({"type":"ephemeral"}));
    assert!(b["system"][0].get("cache_control").is_none());
}

#[test]
fn message_anchor_resolves_through_the_system_hoist_skip() {
    // A leading System message is hoisted out, so canonical index 1 (the user turn)
    // lands at WIRE position 0 — proving wire_pos != canonical index.
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "messages":[
            {"role":"system","content":[{"type":"text","text":"sys"}]},
            {"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}
        ],
        "cache":[{"anchor":"message","index":1}]
    })));
    assert_eq!(b["messages"].as_array().unwrap().len(), 1); // system hoisted
    assert_eq!(
        b["messages"][0]["content"][1]["cache_control"],
        json!({"type":"ephemeral"})
    );
    assert!(b["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());
}

#[test]
fn one_hour_ttl_emits_the_1h_spelling() {
    let b = body(&from(json!({
        "model":"x","max_tokens":1,
        "tools":[{"name":"a","input_schema":{}}],
        "cache":[{"anchor":"tools","ttl":"1h"}]
    })));
    assert_eq!(
        b["tools"][0]["cache_control"],
        json!({"type":"ephemeral","ttl":"1h"})
    );
}

#[test]
fn more_than_four_breakpoints_is_parse_input() {
    let err = enc(&from(json!({
        "model":"x","max_tokens":1,
        "tools":[{"name":"a","input_schema":{}}],
        "cache":[
            {"anchor":"tools"},{"anchor":"tools"},{"anchor":"tools"},
            {"anchor":"tools"},{"anchor":"tools"}
        ]
    })))
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ParseInput);
    assert_eq!(err.exit_code(), 64);
}

#[test]
fn unresolvable_anchors_are_parse_input() {
    // Tools anchor with no tools (last_of None).
    let parse = |v: Value| {
        let e = enc(&from(v)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::ParseInput);
        assert_eq!(e.exit_code(), 64);
    };
    parse(json!({"model":"x","max_tokens":1,"cache":[{"anchor":"tools"}]}));
    // System anchor with no system (last_of None).
    parse(json!({"model":"x","max_tokens":1,"cache":[{"anchor":"system"}]}));
    // Message index out of range.
    parse(json!({"model":"x","max_tokens":1,
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],
        "cache":[{"anchor":"message","index":5}]}));
    // Message index at a hoisted System-role message.
    parse(json!({"model":"x","max_tokens":1,
        "messages":[{"role":"system","content":[{"type":"text","text":"sys"}]}],
        "cache":[{"anchor":"message","index":0}]}));
    // Message index at a 0-block message: an assistant turn of only signature-less
    // `Thinking`, which `content_block` drops → empty content array, last_mut None.
    parse(json!({"model":"x","max_tokens":1,
        "messages":[{"role":"assistant",
            "content":[{"type":"thinking","text":"t","signature":null}]}],
        "cache":[{"anchor":"message","index":0}]}));
}
