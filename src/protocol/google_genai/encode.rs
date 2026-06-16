//! REQUEST projection (providers §4.2): canonical → the `generateContent` wire.
//! The model selects the URL path (`models/{model}:…`), roles are `user`/`model`,
//! generation params nest under `generationConfig`, and `system` hoists to
//! `systemInstruction`. Pure; the `x-goog-api-key` header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Role, Tool, ToolChoice,
};
use crate::protocol::{ProviderCtx, WireRequest};

/// Build the wire request (§4.2). Streaming is the endpoint CHOICE
/// (`:streamGenerateContent?alt=sse` vs `:generateContent`), not a body field.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    if let Some(si) = system_instruction(req)? {
        body.insert("systemInstruction".into(), si);
    }
    body.insert("contents".into(), contents_value(req)?);
    if !req.tools.is_empty() {
        body.insert(
            "tools".into(),
            json!([{ "functionDeclarations": fn_decls(&req.tools) }]),
        );
    }
    if let Some(tc) = tool_config(&req.tool_choice) {
        body.insert("toolConfig".into(), tc);
    }
    let gen = generation_config(req);
    if !gen.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen));
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§4.2)
    }
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let verb = if req.stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };
    let url = format!("{}/v1beta/models/{}:{}", ctx.base_url, ctx.model, verb);
    let mut wire = WireRequest::new(url, bytes);
    wire.set_header("content-type", "application/json");
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    Ok(wire)
}

/// A text-only wire slot rejected non-text content (§4.2).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
    }
}

/// `system` + any `Role::System` message hoist to one `systemInstruction` (§4.3) —
/// Google has no system role inline. `None` when there is no system text at all.
fn system_instruction(req: &CanonicalRequest) -> Result<Option<Value>, CanonicalError> {
    let mut text = String::new();
    if let Some(s) = &req.system {
        text.push_str(&concat_text(s, "system")?);
    }
    for m in &req.messages {
        if m.role == Role::System {
            text.push_str(&concat_text(&m.content, "system")?);
        }
    }
    Ok((!text.is_empty()).then(|| json!({ "parts": [{ "text": text }] })))
}

/// Project non-system messages to `contents[]` (§4.3): `user`/`model` roles, a
/// `Role::Tool` riding a `user` turn carrying `functionResponse` parts.
fn contents_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for m in &req.messages {
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
            Role::System => continue, // hoisted to systemInstruction
        };
        out.push(json!({ "role": role, "parts": parts_value(&m.content)? }));
    }
    Ok(Value::Array(out))
}

/// One message's content → Google `parts[]` (§4.3). `Thinking`/`RedactedThinking`
/// drop (empty-set rule); everything else maps structurally.
fn parts_value(content: &[Content]) -> Result<Value, CanonicalError> {
    let mut parts = Vec::new();
    for c in content {
        match c {
            Content::Text(t) => parts.push(json!({ "text": t })),
            Content::Image { source } => parts.push(image_part(source)),
            Content::ToolUse { name, input, .. } => {
                parts.push(json!({ "functionCall": { "name": name, "args": input } }))
            }
            Content::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => parts.push(function_response(tool_use_id, content, *is_error)?),
            Content::Thinking { .. } | Content::RedactedThinking { .. } => {} // dropped (§4.3)
        }
    }
    Ok(Value::Array(parts))
}

/// `Image` source → a Google part (§4.3): base64 is STRUCTURED `inlineData`
/// (media-type + data); a URL rides `fileData.fileUri`.
fn image_part(source: &ImageSource) -> Value {
    match source {
        ImageSource::Base64 { media_type, data } => {
            json!({ "inlineData": { "mimeType": media_type, "data": data } })
        }
        ImageSource::Url { url } => json!({ "fileData": { "fileUri": url } }),
    }
}

/// `ToolResult` → a `functionResponse` part (§4.3, §4.5): keyed by NAME (the
/// `tool_use_id` projected back, since Google sends no id); `is_error` surfaces
/// textually. The result text is a text-only slot — non-`Text` rejects.
fn function_response(
    name: &str,
    content: &[Content],
    is_error: bool,
) -> Result<Value, CanonicalError> {
    let mut text = concat_text(content, "tool_result")?;
    if is_error {
        text = format!("[error] {text}");
    }
    Ok(json!({ "functionResponse": { "name": name, "response": { "result": text } } }))
}

/// Flatten text-only content to a plain string (§4.2/§4.3); any non-`Text` rejects.
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

/// `tools[]` → `functionDeclarations` (§4.2); `description` omitted when `None`,
/// `parameters` ← `input_schema` verbatim.
fn fn_decls(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                let mut d = json!({ "name": t.name, "parameters": t.input_schema });
                if let Some(desc) = &t.description {
                    d["description"] = json!(desc);
                }
                d
            })
            .collect(),
    )
}

/// `tool_choice` → `toolConfig.functionCallingConfig` (§4.2): `Auto` omits (the
/// default `AUTO`); `Any`→`ANY`; `None`→`NONE`; `Tool{name}`→`ANY` + allow-list.
fn tool_config(tc: &ToolChoice) -> Option<Value> {
    let cfg = match tc {
        ToolChoice::Auto => return None,
        ToolChoice::Any => json!({ "mode": "ANY" }),
        ToolChoice::None => json!({ "mode": "NONE" }),
        ToolChoice::Tool { name } => {
            json!({ "mode": "ANY", "allowedFunctionNames": [name] })
        }
    };
    Some(json!({ "functionCallingConfig": cfg }))
}

/// Generation params → the nested `generationConfig` (§4.2), each omitted when
/// absent so an empty config is dropped entirely.
fn generation_config(req: &CanonicalRequest) -> Map<String, Value> {
    let mut gen = Map::new();
    if let Some(n) = req.max_tokens {
        gen.insert("maxOutputTokens".into(), json!(n));
    }
    if let Some(t) = req.temperature {
        gen.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        gen.insert("topP".into(), json!(p));
    }
    if !req.stop.is_empty() {
        gen.insert("stopSequences".into(), json!(req.stop)); // RENAME + nesting
    }
    gen
}
