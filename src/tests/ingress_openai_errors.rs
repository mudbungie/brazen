//! `openai_chat` ingress rung-4 rejections (ingress.md §3): every structural
//! impossibility rejects with a message NAMING the offending path — never a silent
//! drop, never provider policy pre-enforced. Also the one `IngressError` →
//! `CanonicalError` projection (always `ParseInput`, brazen as origin).

use serde_json::{json, Value};

use crate::{decode_request, CanonicalError, ErrorKind, IngressId};

/// Decode a body expected to reject; return its message for the path assertion.
fn fail(v: Value) -> String {
    decode_request(IngressId::OpenAiChat, v.to_string().as_bytes())
        .unwrap_err()
        .message
}

fn assert_names(msg: &str, path: &str) {
    assert!(
        msg.contains(path) && msg.starts_with("openai_chat ingress:"),
        "message {msg:?} does not name `{path}`"
    );
}

#[test]
fn ingress_error_projects_to_parse_input() {
    let e = decode_request(IngressId::OpenAiChat, b"not json").unwrap_err();
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
    assert_names(
        &fail(json!({"parallel_tool_calls": "yes"})),
        "parallel_tool_calls",
    );
    assert_names(&fail(json!({"max_tokens": -1})), "max_tokens");
    assert_names(&fail(json!({"max_tokens": 1.5})), "max_tokens");
    assert_names(
        &fail(json!({"max_completion_tokens": 4294967296u64})),
        "max_completion_tokens",
    );
    assert_names(&fail(json!({"temperature": "hot"})), "temperature");
    assert_names(&fail(json!({"top_p": true})), "top_p");
    assert_names(&fail(json!({"stream": 1})), "stream");
    assert_names(&fail(json!({"stop": 7})), "stop");
    assert_names(&fail(json!({"stop": ["ok", 7]})), "stop[1]");
    assert_names(&fail(json!({"reasoning_effort": 3})), "reasoning_effort");
    // an effort outside the closed canonical set has no projection (rung 4)
    assert_names(
        &fail(json!({"reasoning_effort": "minimal"})),
        "reasoning_effort",
    );
}

#[test]
fn tools_shapes() {
    assert_names(&fail(json!({"tools": {}})), "tools");
    assert_names(&fail(json!({"tools": ["x"]})), "tools[0]");
    assert_names(&fail(json!({"tools": [{"type": 1}]})), "tools[0].type");
    assert_names(
        &fail(json!({"tools": [{"type": "custom"}]})),
        "tools[0].type",
    );
    assert_names(
        &fail(json!({"tools": [{"type": "function"}]})),
        "tools[0].function",
    );
    assert_names(
        &fail(json!({"tools": [{"function": {"name": 1}}]})),
        "tools[0].function.name",
    );
    assert_names(
        &fail(json!({"tools": [{"function": {"name": "f", "description": 1}}]})),
        "tools[0].function.description",
    );
    assert_names(
        &fail(json!({"tools": [{"function": {"name": "f", "strict": "yes"}}]})),
        "tools[0].function.strict",
    );
}

#[test]
fn tool_choice_shapes() {
    assert_names(&fail(json!({"tool_choice": 1})), "tool_choice");
    assert_names(&fail(json!({"tool_choice": "sometimes"})), "tool_choice");
    assert_names(
        &fail(json!({"tool_choice": {"type": "allowed_tools"}})),
        "tool_choice.type",
    );
    assert_names(
        &fail(json!({"tool_choice": {"type": "function"}})),
        "tool_choice.function",
    );
    assert_names(
        &fail(json!({"tool_choice": {"type": "function", "function": {}}})),
        "tool_choice.function.name",
    );
}

#[test]
fn response_format_shapes() {
    assert_names(&fail(json!({"response_format": "json"})), "response_format");
    assert_names(
        &fail(json!({"response_format": {}})),
        "response_format.type",
    );
    assert_names(
        &fail(json!({"response_format": {"type": "grammar"}})),
        "response_format.type",
    );
    assert_names(
        &fail(json!({"response_format": {"type": "json_schema"}})),
        "response_format.json_schema",
    );
    assert_names(
        &fail(json!({"response_format": {"type": "json_schema", "json_schema": {"name": 1}}})),
        "response_format.json_schema.name",
    );
    assert_names(
        &fail(json!({"response_format": {"type": "json_schema", "json_schema": {"strict": 1}}})),
        "response_format.json_schema.strict",
    );
}

#[test]
fn message_shapes() {
    let msg = |m: Value| fail(json!({"messages": [m]}));
    assert_names(&fail(json!({"messages": {}})), "messages");
    assert_names(&fail(json!({"messages": ["x"]})), "messages[0]");
    assert_names(&msg(json!({"content": "hi"})), "messages[0].role");
    assert_names(
        &msg(json!({"role": "narrator", "content": "hi"})),
        "messages[0].role",
    );
    assert_names(&msg(json!({"role": "user"})), "messages[0].content");
    assert_names(
        &msg(json!({"role": "user", "content": 7})),
        "messages[0].content",
    );
    assert_names(
        &msg(json!({"role": "user", "content": ["x"]})),
        "messages[0].content[0]",
    );
    assert_names(
        &msg(json!({"role": "user", "content": [{"text": "hi"}]})),
        "messages[0].content[0].type",
    );
    assert_names(
        &msg(json!({"role": "user", "content": [{"type": "input_audio"}]})),
        "messages[0].content[0].type",
    );
    assert_names(
        &msg(json!({"role": "user", "content": [{"type": "text"}]})),
        "messages[0].content[0].text",
    );
    // media parts in a text-only slot (system here) fall to the same named reject
    assert_names(
        &msg(
            json!({"role": "system", "content": [{"type": "image_url", "image_url": {"url": "u"}}]}),
        ),
        "messages[0].content[0].type",
    );
}

#[test]
fn media_part_shapes() {
    let user = |p: Value| fail(json!({"messages": [{"role": "user", "content": [p]}]}));
    assert_names(
        &user(json!({"type": "image_url"})),
        "messages[0].content[0].image_url",
    );
    assert_names(
        &user(json!({"type": "image_url", "image_url": {}})),
        "messages[0].content[0].image_url.url",
    );
    // a file part without base64 file_data (an uploaded file_id) has no canonical slot
    assert_names(
        &user(json!({"type": "file", "file": {"file_id": "file-1"}})),
        "messages[0].content[0].file",
    );
    assert_names(
        &user(json!({"type": "file"})),
        "messages[0].content[0].file",
    );
    // file_data that is not the data-URI embedding is shapeless
    assert_names(
        &user(json!({"type": "file", "file": {"file_data": "JVBERi0="}})),
        "messages[0].content[0].file.file_data",
    );
}

#[test]
fn assistant_and_tool_shapes() {
    let msg = |m: Value| fail(json!({"messages": [m]}));
    assert_names(
        &msg(json!({"role": "assistant", "content": 7})),
        "messages[0].content",
    );
    assert_names(
        &msg(json!({"role": "assistant", "tool_calls": {}})),
        "messages[0].tool_calls",
    );
    assert_names(
        &msg(json!({"role": "assistant", "tool_calls": ["x"]})),
        "messages[0].tool_calls[0]",
    );
    let call = |c: Value| msg(json!({"role": "assistant", "tool_calls": [c]}));
    assert_names(&call(json!({"type": "custom"})), "tool_calls[0].type");
    assert_names(
        &call(json!({"id": "c", "function": {"arguments": "{}"}})),
        "tool_calls[0].function.name",
    );
    assert_names(
        &call(json!({"function": {"name": "f", "arguments": "{}"}})),
        "tool_calls[0].id",
    );
    assert_names(&call(json!({"id": "c"})), "tool_calls[0].function");
    assert_names(
        &call(json!({"id": "c", "function": {"name": "f", "arguments": "{oops"}})),
        "tool_calls[0].function.arguments",
    );
    assert_names(
        &call(json!({"id": "c", "function": {"name": "f", "arguments": 7}})),
        "tool_calls[0].function.arguments",
    );
    assert_names(
        &msg(json!({"role": "tool", "content": "r"})),
        "messages[0].tool_call_id",
    );
    assert_names(
        &msg(json!({"role": "tool", "tool_call_id": "c"})),
        "messages[0].content",
    );
}
