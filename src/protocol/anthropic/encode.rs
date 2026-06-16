//! REQUEST projection (anthropic-messages §2): canonical → `POST /v1/messages`
//! body + non-auth headers. Pure; the auth header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Message, Role, Tool,
    ToolChoice,
};
use crate::protocol::{ProviderCtx, WireRequest};

/// Build the wire request (§2.2). Typed fields serialize first; `extra` folds in
/// only keys they did not set — the typed field is the single source of truth.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    // max_tokens is REQUIRED by the API and folded by config resolution; a `None`
    // here is a resolution bug → Config (exit 78), never a silent omit.
    let max_tokens = req.max_tokens.ok_or_else(config_err)?;
    body.insert("max_tokens".into(), json!(max_tokens));
    if let Some(system) = &req.system {
        body.insert("system".into(), system_value(system)?); // hoisted top-level
    }
    body.insert("messages".into(), messages_value(&req.messages)?);
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        body.insert("stop_sequences".into(), json!(req.stop)); // rename: stop → stop_sequences
    }
    body.insert("stream".into(), json!(req.stream));
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools));
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice, req.tools.is_empty()) {
        body.insert("tool_choice".into(), tc);
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§2.1.1)
    }
    // Our own owned Map of Values serializes infallibly (mirrors NdjsonSink §5.2).
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(format!("{}/v1/messages", ctx.base_url), bytes);
    wire.set_header("content-type", "application/json");
    // anthropic-version (and any beta) ride ctx.beta_headers verbatim, never hard-coded.
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    Ok(wire)
}

fn config_err() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: "anthropic_messages requires max_tokens".into(),
        provider_detail: None,
    }
}

/// A text-only / text-image-only wire slot rejected non-representable content (§2.4/§2.5).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
    }
}

/// `req.system` → top-level `system`: a text-only slot (§2.4). Always the array
/// form — wire-equivalent to the bare string and never loses caching.
fn system_value(system: &[Content]) -> Result<Value, CanonicalError> {
    let mut blocks = Vec::new();
    for c in system {
        match c {
            Content::Text(t) => blocks.push(json!({"type": "text", "text": t})),
            _ => return Err(slot_err("system")),
        }
    }
    Ok(Value::Array(blocks))
}

/// Project `messages[]` (§2.3): `System` is hoisted to top-level `system` (never
/// inline); `Tool` becomes a `"user"` message carrying `tool_result` blocks.
fn messages_value(msgs: &[Message]) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for m in msgs {
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue,
        };
        let mut blocks = Vec::new();
        for c in &m.content {
            if let Some(b) = content_block(c)? {
                blocks.push(b);
            }
        }
        out.push(json!({"role": role, "content": Value::Array(blocks)}));
    }
    Ok(Value::Array(out))
}

/// One `Content` → one wire ContentBlockParam (§2.5). `Ok(None)` drops a block
/// that cannot be replayed (a signature-less `Thinking`, CR-2).
fn content_block(c: &Content) -> Result<Option<Value>, CanonicalError> {
    Ok(Some(match c {
        Content::Text(t) => json!({"type": "text", "text": t}),
        Content::Image { source } => json!({"type": "image", "source": image_source(source)}),
        Content::ToolUse { id, name, input } => {
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        Content::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut v = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": tool_result_content(content)?,
            });
            if *is_error {
                v["is_error"] = json!(true); // omitted when false
            }
            v
        }
        Content::Thinking {
            text,
            signature: Some(sig),
        } => json!({"type": "thinking", "thinking": text, "signature": sig}),
        Content::Thinking {
            signature: None, ..
        } => return Ok(None),
        Content::RedactedThinking { data } => json!({"type": "redacted_thinking", "data": data}),
    }))
}

fn image_source(s: &ImageSource) -> Value {
    match s {
        ImageSource::Base64 { media_type, data } => {
            json!({"type": "base64", "media_type": media_type, "data": data})
        }
        ImageSource::Url { url } => json!({"type": "url", "url": url}),
    }
}

/// `tool_result.content` (§2.5): a text/image-only slot — any other nested
/// `Content` is unrepresentable and rejects with `ParseInput`.
fn tool_result_content(content: &[Content]) -> Result<Value, CanonicalError> {
    let mut blocks = Vec::new();
    for c in content {
        match c {
            Content::Text(t) => blocks.push(json!({"type": "text", "text": t})),
            Content::Image { source } => {
                blocks.push(json!({"type": "image", "source": image_source(source)}))
            }
            _ => return Err(slot_err("tool_result")),
        }
    }
    Ok(Value::Array(blocks))
}

/// Flat custom-tool objects (§2.6); `description` omitted when `None`.
fn tools_value(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let mut o = json!({"name": t.name, "input_schema": t.input_schema});
                if let Some(d) = &t.description {
                    o["description"] = json!(d);
                }
                o
            })
            .collect(),
    )
}

/// `tool_choice` (§2.7): `Auto` is omitted entirely when there are no tools.
fn tool_choice_value(tc: &ToolChoice, no_tools: bool) -> Option<Value> {
    Some(match tc {
        ToolChoice::Auto if no_tools => return None,
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::Any => json!({"type": "any"}),
        ToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
        ToolChoice::None => json!({"type": "none"}),
    })
}
