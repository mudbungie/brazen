//! `anthropic_messages` ingress rung-4 rejections (ingress.md §3): every structural
//! impossibility rejects with a message NAMING the offending path — never a silent
//! drop, never provider policy pre-enforced.

use serde_json::{json, Value};

use crate::{decode_request, CanonicalError, ErrorKind, IngressId};

/// Decode a body expected to reject; return its message for the path assertion.
fn fail(v: Value) -> String {
    decode_request(IngressId::AnthropicMessages, v.to_string().as_bytes())
        .unwrap_err()
        .message
}

fn assert_names(msg: &str, path: &str) {
    assert!(
        msg.contains(path) && msg.starts_with("anthropic_messages ingress:"),
        "message {msg:?} does not name `{path}`"
    );
}

/// A body with `messages` wrapping one assistant content block, for per-block paths.
fn block(b: Value) -> String {
    fail(json!({"model": "m", "messages": [{"role": "assistant", "content": [b]}]}))
}

#[test]
fn ingress_error_projects_to_parse_input() {
    let e = decode_request(IngressId::AnthropicMessages, b"not json").unwrap_err();
    assert!(e.message.contains("request body is not JSON"));
    let c = CanonicalError::from(e.clone());
    assert_eq!(c.kind, ErrorKind::ParseInput);
    assert_eq!(c.message, e.message);
    assert_eq!(c.provider_detail, None);
    assert_eq!(c.retry_after_seconds, None);
}

#[test]
fn top_level_shapes() {
    assert_names(&fail(json!(["not", "an", "object"])), "request body");
    assert_names(&fail(json!({"model": 5})), "model");
    assert_names(&fail(json!({"max_tokens": -1})), "max_tokens");
    assert_names(&fail(json!({"max_tokens": 1.5})), "max_tokens");
    assert_names(&fail(json!({"temperature": "hot"})), "temperature");
    assert_names(&fail(json!({"top_p": true})), "top_p");
    assert_names(&fail(json!({"stream": 1})), "stream");
    assert_names(&fail(json!({"stop_sequences": 7})), "stop_sequences");
    assert_names(
        &fail(json!({"stop_sequences": ["ok", 7]})),
        "stop_sequences[1]",
    );
}

#[test]
fn tools_and_tool_choice_shapes() {
    assert_names(&fail(json!({"tools": 7})), "tools");
    assert_names(&fail(json!({"tools": [7]})), "tools[0]");
    assert_names(&fail(json!({"tools": [{"type": 5}]})), "tools[0].type");
    assert_names(&fail(json!({"tools": [{"type": "x"}]})), "tools[0].name"); // provider needs name
    assert_names(&fail(json!({"tools": [{}]})), "tools[0].name"); // custom needs name
    assert_names(
        &fail(json!({"tools": [{"name": "n", "description": 5}]})),
        "tools[0].description",
    );
    assert_names(
        &fail(json!({"tools": [{"name": "n", "strict": 5}]})),
        "tools[0].strict",
    );
    assert_names(&fail(json!({"tool_choice": 7})), "tool_choice");
    assert_names(&fail(json!({"tool_choice": {}})), "tool_choice.type");
    assert_names(
        &fail(json!({"tool_choice": {"type": "mystery"}})),
        "tool_choice.type",
    );
    assert_names(
        &fail(json!({"tool_choice": {"type": "tool"}})),
        "tool_choice.name",
    );
}

#[test]
fn output_config_shapes() {
    assert_names(&fail(json!({"output_config": 7})), "output_config");
    assert_names(
        &fail(json!({"output_config": {"format": 7}})),
        "output_config.format",
    );
    assert_names(
        &fail(json!({"output_config": {"format": {"type": "json_object"}}})),
        "output_config.format.type",
    );
}

#[test]
fn message_and_system_shapes() {
    assert_names(&fail(json!({"messages": 7})), "messages");
    assert_names(&fail(json!({"messages": [7]})), "messages[0]");
    assert_names(
        &fail(json!({"messages": [{"content": []}]})),
        "messages[0].role",
    );
    assert_names(
        &fail(json!({"messages": [{"role": "tool", "content": []}]})),
        "messages[0].role",
    );
    assert_names(
        &fail(json!({"messages": [{"role": "user"}]})),
        "messages[0].content",
    );
    assert_names(&fail(json!({"system": 7})), "system");
    assert_names(&fail(json!({"system": [7]})), "system[0]");
    assert_names(
        &fail(json!({"system": [{"type": "image"}]})),
        "system[0].type",
    );
    assert_names(
        &fail(json!({"system": [{"type": "text"}]})),
        "system[0].text",
    );
}

#[test]
fn content_block_shapes() {
    assert_names(&block(json!(7)), "content[0]");
    assert_names(&block(json!({})), "content[0].type");
    assert_names(&block(json!({"type": "text"})), "content[0].text");
    assert_names(&block(json!({"type": "mystery"})), "content[0].type");
    assert_names(
        &block(json!({"type": "tool_use", "name": "f"})),
        "content[0].id",
    );
    assert_names(
        &block(json!({"type": "tool_use", "id": "t"})),
        "content[0].name",
    );
    assert_names(&block(json!({"type": "thinking"})), "content[0].thinking");
    assert_names(
        &block(json!({"type": "redacted_thinking"})),
        "content[0].data",
    );
    assert_names(
        &block(json!({"type": "server_tool_use", "name": "s"})),
        "content[0].id",
    );
    assert_names(
        &block(json!({"type": "web_search_tool_result"})),
        "content[0].tool_use_id",
    );
}

#[test]
fn image_document_and_tool_result_slot_shapes() {
    assert_names(&block(json!({"type": "image"})), "content[0].source");
    assert_names(
        &block(json!({"type": "image", "source": {"type": "x"}})),
        "content[0].source.type",
    );
    assert_names(
        &block(json!({"type": "image", "source": {"type": "base64", "data": "d"}})),
        "content[0].source.media_type",
    );
    assert_names(
        &block(json!({"type": "image", "source": {"type": "url"}})),
        "content[0].source.url",
    );
    assert_names(&block(json!({"type": "document"})), "content[0].source");
    assert_names(
        &block(json!({"type": "document", "source": {"type": "x"}})),
        "content[0].source.type",
    );
    assert_names(
        &block(
            json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf"}}),
        ),
        "content[0].source.data",
    );
    assert_names(
        &block(json!({"type": "document", "source": {"type": "url"}})),
        "content[0].source.url",
    );
    // the tool_result content slot rejects a non-text/image nested block, and a bad shape
    assert_names(
        &block(json!({"type": "tool_result", "tool_use_id": "t",
            "content": [{"type": "tool_use", "id": "x", "name": "y"}]})),
        "content[0].content[0].type",
    );
    assert_names(
        &block(json!({"type": "tool_result", "tool_use_id": "t", "content": 7})),
        "content[0].content",
    );
    assert_names(
        &block(json!({"type": "tool_result", "tool_use_id": "t", "content": [{"type": "text"}]})),
        "content[0].content[0].text",
    );
}
