//! The `contents[]` + `systemInstruction` projection of the Google request (§4.3):
//! `system` and any `Role::System` message hoist to one `systemInstruction`; the
//! rest become `user`/`model` turns, with `Role::Tool` riding a `user` turn of
//! `functionResponse` parts. `super::encode` calls [`system_instruction`] and
//! [`contents_value`]; the text-only slot rejection (`slot_err`) lives here.

use serde_json::{json, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, DocumentSource, ErrorKind, ImageSource, Role,
};

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
            Content::Image { source } => parts.push(image_part(source)?),
            Content::Document { source } => parts.push(document_part(source)?),
            Content::ToolUse {
                name,
                input,
                signature,
                ..
            } => {
                // Echo the LOAD-BEARING thoughtSignature as a sibling of functionCall
                // when present — Gemini 2.5 multi-turn function calling 400s without it
                // (§4.3, bl-61a9). Absent when None (Anthropic/OpenAI tool calls).
                let mut part = json!({ "functionCall": { "name": name, "args": input } });
                if let Some(sig) = signature {
                    part["thoughtSignature"] = json!(sig);
                }
                parts.push(part);
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
/// (media-type + data). An `Image{Url}` (a web URL) has **no Gemini wire home** and
/// REJECTS at encode (architecture.md §3.1, providers.md §4.3 CR-G3): Gemini's
/// `fileData.fileUri` references only files already uploaded to the Google Files API
/// (not arbitrary `https://…` URLs, which it cannot fetch) and generally wants a
/// `mimeType` sibling brazen cannot infer from a URL — so a total reject, not a
/// prefix-sniffing narrowing. The image analogue of Ollama's base64-only slot (§5.4
/// CR-O2); the remedy — the caller downloads and re-sends as base64 (`inlineData`) —
/// is named in the message, never a brazen-added round-trip (architecture.md §2).
fn image_part(source: &ImageSource) -> Result<Value, CanonicalError> {
    match source {
        ImageSource::Base64 { media_type, data } => {
            Ok(json!({ "inlineData": { "mimeType": media_type, "data": data } }))
        }
        ImageSource::Url { .. } => Err(url_image_err()),
    }
}

/// `Document` source → a Google part (§4.3): base64 is STRUCTURED `inlineData`
/// (`application/pdf` etc.), the same shape as an image. A `Document{Url}` REJECTS at
/// encode (CR-G3, the same rule as `Image{Url}`): Gemini's `fileData.fileUri` references
/// only Google Files-API/GCS URIs, not arbitrary web URLs, and wants a `mimeType` brazen
/// cannot infer from a URL — a total reject, remedy named in the message.
fn document_part(source: &DocumentSource) -> Result<Value, CanonicalError> {
    match source {
        DocumentSource::Base64 { media_type, data } => {
            Ok(json!({ "inlineData": { "mimeType": media_type, "data": data } }))
        }
        DocumentSource::Url { .. } => Err(url_document_err()),
    }
}

/// An `Image{Url}` rejected: Gemini has no web-URL image slot (§4.3). The message
/// names the limitation and the remedy (re-send as base64) — no accepted URL form.
fn url_image_err() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: "google: image URLs are not supported — Gemini's fileData.fileUri \
                  references only files uploaded to the Google Files API, not web URLs, \
                  and needs a mimeType brazen cannot infer from a URL; download the \
                  image and re-send it as a base64 image"
            .to_string(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// A `Document{Url}` rejected: Gemini has no web-URL document slot (§4.3, CR-G3), the
/// same rule as `Image{Url}` — download and re-send as a base64 document (`inlineData`).
fn url_document_err() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: "google: document URLs are not supported — Gemini's fileData.fileUri \
                  references only files uploaded to the Google Files API, not web URLs, \
                  and needs a mimeType brazen cannot infer from a URL; download the \
                  document and re-send it as a base64 document"
            .to_string(),
        provider_detail: None,
        retry_after_seconds: None,
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
        retry_after_seconds: None,
    }
}
