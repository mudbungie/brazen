//! REQUEST projection (openai-chat-mapping §2): canonical → `POST
//! {base_url}/chat/completions` body + non-auth headers. Pure; the
//! `Authorization: Bearer` header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Role, Tool, ToolChoice,
};
use crate::protocol::{ProviderCtx, WireRequest};

/// Build the wire request (§2.1). Typed fields serialize first; `extra` folds in
/// only keys they did not set — the typed field is the single source of truth.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    body.insert("messages".into(), messages_value(req)?);
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools)); // omit when empty
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice) {
        body.insert("tool_choice".into(), tc); // Auto omitted (OpenAI default)
    }
    if let Some(n) = req.max_tokens {
        body.insert("max_tokens".into(), json!(n)); // None → omit (row requires none)
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        body.insert("stop".into(), json!(req.stop)); // array form; omit when empty
    }
    body.insert("stream".into(), json!(req.stream));
    if req.stream {
        // Without include_usage a streamed response carries ZERO usage (§2.8).
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§2.1.1)
    }
    // Our own owned Map of Values serializes infallibly (mirrors the Anthropic encode).
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(format!("{}/chat/completions", ctx.base_url), bytes);
    wire.set_header("content-type", "application/json");
    // Built-in OpenAI row defines no beta headers; a Mistral-style row may — ride
    // ctx.beta_headers verbatim, never hard-coded, never branched on a vendor name.
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    Ok(wire)
}

/// A text-only wire slot (system, tool message) rejected non-text content (§2.3/§2.4).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
    }
}

/// Project `messages[]` (§2.2): the `system` field is prepended as one leading
/// `role:"system"` message; each `Message` then projects per its role, with a
/// `Role::Tool` fanning out to one `role:"tool"` message per `ToolResult` (§2.4).
fn messages_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    if let Some(system) = req.system.as_ref().filter(|s| !s.is_empty()) {
        out.push(json!({"role": "system", "content": content_value(system, false, "system")?}));
    }
    for m in &req.messages {
        match m.role {
            Role::System => out.push(
                json!({"role": "system", "content": content_value(&m.content, false, "system")?}),
            ),
            Role::User => out
                .push(json!({"role": "user", "content": content_value(&m.content, true, "user")?})),
            Role::Assistant => out.push(assistant_message(&m.content)?),
            Role::Tool => tool_messages(&m.content, &mut out)?,
        }
    }
    Ok(Value::Array(out))
}

/// An assistant message (§2.2): `ToolUse` parts collect into `tool_calls`,
/// `Thinking`/`RedactedThinking` are dropped (§2.9), and the rest render as
/// `content` — omitted entirely (never `""`) when there is no text but there are
/// tool calls.
fn assistant_message(content: &[Content]) -> Result<Value, CanonicalError> {
    let mut parts = Vec::new();
    let mut calls = Vec::new();
    for c in content {
        match c {
            Content::ToolUse { id, name, input } => calls.push(json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": to_json_string(input)},
            })),
            Content::Thinking { .. } | Content::RedactedThinking { .. } => {} // dropped (§2.9)
            other => parts.push(other.clone()),
        }
    }
    let mut obj = Map::new();
    obj.insert("role".into(), json!("assistant"));
    if !calls.is_empty() {
        obj.insert("tool_calls".into(), Value::Array(calls));
    }
    if !parts.is_empty() {
        obj.insert("content".into(), content_value(&parts, true, "assistant")?);
    } else if obj.get("tool_calls").is_none() {
        obj.insert("content".into(), json!("")); // empty assistant turn: content nullable, "" here
    }
    Ok(Value::Object(obj))
}

/// `Role::Tool` → one `{role:"tool"}` message per `ToolResult` (§2.4), keyed by
/// `tool_call_id`. `is_error` has no native field — surfaced textually by a
/// `"[error] "` content prefix (§2.4, CR-3).
fn tool_messages(content: &[Content], out: &mut Vec<Value>) -> Result<(), CanonicalError> {
    for c in content {
        let Content::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = c
        else {
            return Err(slot_err("tool"));
        };
        let mut text = tool_result_text(content)?;
        if *is_error {
            text = format!("[error] {text}");
        }
        out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": text}));
    }
    Ok(())
}

/// Render text/image parts into an OpenAI `content` value (§2.2): a single `Text`
/// part is a bare string (the common shape); otherwise the array form. A
/// text-only slot (`allow_image == false`) rejects any image as `ParseInput`.
fn content_value(
    parts: &[Content],
    allow_image: bool,
    slot: &str,
) -> Result<Value, CanonicalError> {
    if let [Content::Text(t)] = parts {
        return Ok(json!(t)); // bare string for a lone text part
    }
    let mut arr = Vec::new();
    for p in parts {
        match p {
            Content::Text(t) => arr.push(json!({"type": "text", "text": t})),
            Content::Image { source } if allow_image => {
                arr.push(json!({"type": "image_url", "image_url": image_url(source)}))
            }
            _ => return Err(slot_err(slot)),
        }
    }
    Ok(Value::Array(arr))
}

/// `tool_result.content` flattened to plain text (§2.4): a text-only slot, so any
/// non-`Text` nested content rejects with `ParseInput`.
fn tool_result_text(content: &[Content]) -> Result<String, CanonicalError> {
    let mut text = String::new();
    for c in content {
        match c {
            Content::Text(t) => text.push_str(t),
            _ => return Err(slot_err("tool_result")),
        }
    }
    Ok(text)
}

/// `Image` source → OpenAI `image_url` (§2.2): a base64 image embeds as a data-URI
/// string inside `url` (CR-1, round-trips); a URL passes through.
fn image_url(source: &ImageSource) -> Value {
    match source {
        ImageSource::Base64 { media_type, data } => {
            json!({"url": format!("data:{media_type};base64,{data}")})
        }
        ImageSource::Url { url } => json!({"url": url}),
    }
}

/// `tools[]` → nested function objects (§2.5); `description` omitted when `None`,
/// `parameters` carries the schema verbatim.
fn tools_value(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let mut f = json!({"name": t.name, "parameters": t.input_schema});
                if let Some(d) = &t.description {
                    f["description"] = json!(d);
                }
                json!({"type": "function", "function": f})
            })
            .collect(),
    )
}

/// `tool_choice` spellings (§2.6): `Auto` is omitted (OpenAI's own default); the
/// rest emit explicit values — note `Any` → `"required"`.
fn tool_choice_value(tc: &ToolChoice) -> Option<Value> {
    Some(match tc {
        ToolChoice::Auto => return None,
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({"type": "function", "function": {"name": name}}),
    })
}

/// A tool-call `input` `Value` → its JSON-encoded **string** (§2.2). OpenAI's
/// `arguments` is a string, never a nested object.
fn to_json_string(input: &Value) -> String {
    #[allow(clippy::expect_used)]
    serde_json::to_string(input).expect("a serde_json::Value re-serializes infallibly")
}
