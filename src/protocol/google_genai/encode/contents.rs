//! The `contents[]` + `systemInstruction` projection of the Google request (§4.3):
//! `system` and any `Role::System` message hoist to one `systemInstruction`; the
//! rest become `user`/`model` turns, with `Role::Tool` riding a `user` turn of
//! `functionResponse` parts. `super::encode` calls [`system_instruction`] and
//! [`contents_value`]; the text-only slot rejection (`slot_err`) lives here.

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Role};

/// `system` + any `Role::System` message hoist to one `systemInstruction` (§4.3) —
/// Google has no system role inline. `None` when there is no system text at all.
pub(super) fn system_instruction(req: &CanonicalRequest) -> Result<Option<Value>, CanonicalError> {
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
pub(super) fn contents_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for m in &req.messages {
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
            Role::System => continue, // hoisted to systemInstruction
        };
        out.push(json!({ "role": role, "parts": parts_value(&m.content, req)? }));
    }
    Ok(Value::Array(out))
}

/// One message's content → Google `parts[]` (§4.3). `Thinking`/`RedactedThinking`
/// and the opaque Anthropic server-tool blocks drop (empty-set rule — no Google
/// projection); everything else maps structurally. `req` resolves a `ToolResult`'s
/// function name for the NAME-keyed `functionResponse` (§4.5).
fn parts_value(content: &[Content], req: &CanonicalRequest) -> Result<Value, CanonicalError> {
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
            } => parts.push(function_response(tool_use_id, content, *is_error, req)?),
            Content::Thinking { .. }
            | Content::RedactedThinking { .. }
            | Content::ServerToolUse { .. }
            | Content::ServerToolResult { .. } => {} // dropped (§4.3): no Google projection
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

/// `ToolResult` → a `functionResponse` part (§4.3, §4.5): keyed by NAME — Google
/// matches a result to its call by **function name**, not id (the id it never
/// sent). The name is resolved from the originating `ToolUse` in this request
/// (`req.tool_name`); only if that call is absent (a bare tool-result turn) does
/// it fall back to the `tool_use_id`. `is_error` surfaces textually. The result
/// text is a text-only slot — non-`Text` rejects.
fn function_response(
    tool_use_id: &str,
    content: &[Content],
    is_error: bool,
    req: &CanonicalRequest,
) -> Result<Value, CanonicalError> {
    let name = req.tool_name(tool_use_id).unwrap_or(tool_use_id);
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

/// A text-only wire slot rejected non-text content (§4.2).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
    }
}
