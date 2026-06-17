//! REQUEST projection (providers §5.3): canonical → `POST {base_url}/api/chat`
//! body + non-auth headers. OpenAI-chat-shaped, but generation params nest under
//! `options` and tool-call `arguments` ride as a JSON **object** (not a string).
//! Pure; the `Authorization: Bearer` header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Role, Tool,
};
use crate::protocol::{ProviderCtx, WireRequest};

/// Build the wire request (§5.3). Typed fields serialize first; `extra` folds in
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
    let options = options_value(req);
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options)); // generation params nest here
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false)));
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§5.3)
    }
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(format!("{}/api/chat", ctx.base_url), bytes);
    wire.set_header("content-type", "application/json");
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    Ok(wire)
}

/// `max_tokens`/`temperature`/`top_p`/`stop` → the nested `options` map (§5.3),
/// each omitted when absent so an empty `options` is dropped entirely.
fn options_value(req: &CanonicalRequest) -> Map<String, Value> {
    let mut options = Map::new();
    if let Some(n) = req.max_tokens {
        options.insert("num_predict".into(), json!(n)); // RENAME
    }
    if let Some(t) = req.temperature {
        options.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        options.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        options.insert("stop".into(), json!(req.stop));
    }
    options
}

/// A text-only wire slot rejected non-text content (§5.4).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
    }
}

/// Project `messages[]` (§5.4): the `system` field prepends one `role:"system"`
/// message; each `Message` then projects per its role, a `Role::Tool` fanning out
/// to one `role:"tool"` message per `ToolResult`.
fn messages_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    if let Some(system) = req.system.as_ref().filter(|s| !s.is_empty()) {
        out.push(json!({"role": "system", "content": concat_text(system, "system")?}));
    }
    for m in &req.messages {
        match m.role {
            Role::System => {
                out.push(json!({"role": "system", "content": concat_text(&m.content, "system")?}))
            }
            Role::User => out.push(user_message(&m.content)?),
            Role::Assistant => out.push(assistant_message(&m.content)?),
            Role::Tool => tool_messages(&m.content, req, &mut out)?,
        }
    }
    Ok(Value::Array(out))
}

/// A user message (§5.4): text concatenates into `content`; base64 images collect
/// into a bare `images` array; a URL image is UNREPRESENTABLE (base64-only slot).
fn user_message(content: &[Content]) -> Result<Value, CanonicalError> {
    let mut text = String::new();
    let mut images = Vec::new();
    for c in content {
        match c {
            Content::Text(t) => text.push_str(t),
            Content::Image { source } => images.push(json!(image_b64(source)?)),
            _ => return Err(slot_err("user")),
        }
    }
    let mut obj = Map::new();
    obj.insert("role".into(), json!("user"));
    obj.insert("content".into(), json!(text));
    if !images.is_empty() {
        obj.insert("images".into(), Value::Array(images));
    }
    Ok(Value::Object(obj))
}

/// An assistant message (§5.4): `ToolUse` parts collect into `tool_calls` with
/// `arguments` as an **object**; `Thinking`/`RedactedThinking` drop; text renders
/// into `content` (always present, possibly empty).
fn assistant_message(content: &[Content]) -> Result<Value, CanonicalError> {
    let mut text = String::new();
    let mut calls = Vec::new();
    for c in content {
        match c {
            Content::Text(t) => text.push_str(t),
            Content::ToolUse { name, input, .. } => {
                calls.push(json!({"function": {"name": name, "arguments": input}}))
            }
            Content::Thinking { .. } | Content::RedactedThinking { .. } => {} // dropped (§5.4)
            _ => return Err(slot_err("assistant")),
        }
    }
    let mut obj = Map::new();
    obj.insert("role".into(), json!("assistant"));
    obj.insert("content".into(), json!(text));
    if !calls.is_empty() {
        obj.insert("tool_calls".into(), Value::Array(calls));
    }
    Ok(Value::Object(obj))
}

/// `Role::Tool` → one `{role:"tool"}` message per `ToolResult` (§5.4). Ollama's
/// tool message carries an optional `tool_name`; the result is name-keyed there
/// (positional order still holds), so emit the function name resolved from the
/// originating `ToolUse` (`req.tool_name`) when present, omitting it only for a
/// bare tool-result turn whose call is not in-band. `is_error` surfaces textually
/// via a `"[error] "` prefix — no native field.
fn tool_messages(
    content: &[Content],
    req: &CanonicalRequest,
    out: &mut Vec<Value>,
) -> Result<(), CanonicalError> {
    for c in content {
        let Content::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = c
        else {
            return Err(slot_err("tool"));
        };
        let mut text = concat_text(content, "tool_result")?;
        if *is_error {
            text = format!("[error] {text}");
        }
        let mut obj = Map::new();
        obj.insert("role".into(), json!("tool"));
        obj.insert("content".into(), json!(text));
        if let Some(name) = req.tool_name(tool_use_id) {
            obj.insert("tool_name".into(), json!(name));
        }
        out.push(Value::Object(obj));
    }
    Ok(())
}

/// Flatten text-only content to a plain string (§5.4); any non-`Text` part rejects.
fn concat_text(content: &[Content], slot: &str) -> Result<String, CanonicalError> {
    let mut text = String::new();
    for c in content {
        match c {
            Content::Text(t) => text.push_str(t),
            _ => return Err(slot_err(slot)),
        }
    }
    Ok(text)
}

/// `Image` source → Ollama image payload (§5.4): base64 is a bare string (the
/// media-type is dropped); a URL has no representation → `ParseInput`.
fn image_b64(source: &ImageSource) -> Result<String, CanonicalError> {
    match source {
        ImageSource::Base64 { data, .. } => Ok(data.clone()),
        ImageSource::Url { .. } => Err(slot_err("image")),
    }
}

/// `tools[]` → OpenAI-chat-shaped function objects (§5.3); `description` omitted
/// when `None`, `parameters` carries the schema verbatim.
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
