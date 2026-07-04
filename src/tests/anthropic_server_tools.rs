//! Anthropic server-tool projection, both directions (CR-4 resolved,
//! anthropic-messages §2.5/§2.6/§3.4): crafted decode frames — `server_tool_use`
//! tracked like `tool_use`, the open-set `*_tool_result` family surfacing its full
//! `content` inline at start — and the encode side: `Tool::Provider` enablement
//! plus verbatim `Content::ServerTool*` replay (never folded into client
//! tool_use/tool_result). No network.

use crate::protocol::anthropic::AnthropicMessages;
use crate::{
    CanonicalError, CanonicalRequest, ContentKind, DecodeState, Delta, Event, Frame, Protocol,
    WireRequest,
};
use serde_json::{json, Value};

/// Decode one streamed frame (a normal SSE block payload) against `state`.
fn dec(v: Value, state: &mut DecodeState) -> Vec<Event> {
    let frame = Frame {
        event: None,
        data: serde_json::to_vec(&v).unwrap(),
        status: None,
    };
    AnthropicMessages.decode(frame, state).unwrap()
}

#[test]
fn server_tool_use_surfaces_with_json_deltas() {
    let mut s = DecodeState::default();
    // server_tool_use now has a canonical ContentKind (CR-4 resolved): it is
    // tracked like tool_use — start carries the identity, the input streams as
    // JsonDeltas, and the stop fires.
    assert_eq!(
        dec(
            json!({"type":"content_block_start","index":3,
                   "content_block":{"type":"server_tool_use","id":"s","name":"web_search",
                                    "input":{}}}),
            &mut s
        ),
        vec![Event::ContentStart {
            index: 3,
            kind: ContentKind::ServerToolUse {
                id: "s".into(),
                name: "web_search".into(),
            },
        }]
    );
    assert_eq!(
        dec(
            json!({"type":"content_block_delta","index":3,
                   "delta":{"type":"input_json_delta","partial_json":"{\"query\":\"x\"}"}}),
            &mut s
        ),
        vec![Event::ContentDelta {
            index: 3,
            delta: Delta::JsonDelta("{\"query\":\"x\"}".into()),
        }]
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop","index":3}), &mut s),
        vec![Event::ContentStop { index: 3 }]
    );
}

#[test]
fn server_tool_result_carries_content_inline_at_start_no_delta() {
    let mut s = DecodeState::default();
    // The full content array rides the START (no delta ever arrives); the wire tag
    // is carried as `kind` verbatim.
    let content = json!([{"type":"web_search_result","url":"https://x","title":"X"}]);
    assert_eq!(
        dec(
            json!({"type":"content_block_start","index":2,
                   "content_block":{"type":"web_search_tool_result","tool_use_id":"s",
                                    "content":content.clone()}}),
            &mut s
        ),
        vec![Event::ContentStart {
            index: 2,
            kind: ContentKind::ServerToolResult {
                kind: "web_search_tool_result".into(),
                tool_use_id: "s".into(),
                content,
            },
        }]
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop","index":2}), &mut s),
        vec![Event::ContentStop { index: 2 }]
    );
}

#[test]
fn server_tool_result_suffix_rule_and_error_object_content() {
    let mut s = DecodeState::default();
    // A tag brazen never enumerated rides the suffix arm, and the opaque Value
    // handles content-as-OBJECT (the error shape), not just the array form.
    let content = json!({"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"});
    assert_eq!(
        dec(
            json!({"type":"content_block_start","index":0,
                   "content_block":{"type":"code_execution_tool_result","tool_use_id":"s2",
                                    "content":content.clone()}}),
            &mut s
        ),
        vec![Event::ContentStart {
            index: 0,
            kind: ContentKind::ServerToolResult {
                kind: "code_execution_tool_result".into(),
                tool_use_id: "s2".into(),
                content,
            },
        }]
    );
}

/// Encode `req` against a fixed Anthropic-shaped ctx and return the wire body.
fn body(v: Value) -> Value {
    let req: CanonicalRequest = serde_json::from_value(v).unwrap();
    let beta = [("anthropic-version", "2023-06-01")];
    let ctx = crate::ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-opus-4-8",
        beta_headers: &beta,
    };
    let wire: Result<WireRequest, CanonicalError> = AnthropicMessages.encode(&req, &ctx);
    serde_json::from_slice(&wire.unwrap().body).unwrap()
}

#[test]
fn provider_tool_emits_type_name_and_config_verbatim() {
    // Tool::Provider (a `type`-keyed wire tool) → {type, name, ...config}: every
    // config key folded verbatim, NO input_schema, NO description (§2.6). The
    // system block absorbs the automatic head cache mark (§2.10) so the tools
    // assertion stays about the tool shape.
    let b = body(json!({"model":"x","max_tokens":1,
        "system":[{"type":"text","text":"s"}],
        "tools":[{"type":"web_search_20250305","name":"web_search","max_uses":5}]}));
    assert_eq!(
        b["tools"][0],
        json!({"type":"web_search_20250305","name":"web_search","max_uses":5})
    );
    assert!(b["tools"][0].get("input_schema").is_none());
    assert!(b["tools"][0].get("description").is_none());
}

#[test]
fn server_tool_blocks_replay_verbatim_never_folded() {
    // Content::ServerToolUse / ServerToolResult pass through byte-faithful (§2.5):
    // never converted to tool_use/tool_result (the litellm-class replay 400). The
    // result's `kind` is the wire `type` — including a tag brazen never enumerated.
    let b = body(json!({"model":"x","max_tokens":1,"messages":[
    {"role":"assistant","content":[
        {"type":"server_tool_use","id":"srvtoolu_1","name":"web_search",
         "input":{"query":"weather NY"}},
        {"type":"web_search_tool_result","tool_use_id":"srvtoolu_1",
         "content":[{"type":"web_search_result","url":"https://x"}]},
        {"type":"code_execution_tool_result","tool_use_id":"srvtoolu_2",
         "content":{"type":"code_execution_tool_result_error","error_code":"unavailable"}}
    ]}]}));
    assert_eq!(
        b["messages"][0]["content"],
        json!([
            {"type":"server_tool_use","id":"srvtoolu_1","name":"web_search",
             "input":{"query":"weather NY"}},
            {"type":"web_search_tool_result","tool_use_id":"srvtoolu_1",
             "content":[{"type":"web_search_result","url":"https://x"}]},
            {"type":"code_execution_tool_result","tool_use_id":"srvtoolu_2",
             "content":{"type":"code_execution_tool_result_error","error_code":"unavailable"}}
        ])
    );
}
