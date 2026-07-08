//! The `messages[]` projection of the OpenAI chat request (§2.2–§2.4): the leading
//! `system` field plus each `Message` by role, with `Role::Tool` fanning out to one
//! `role:"tool"` message per `ToolResult`, and the text/image `content` shaping that
//! every role shares. `super::encode` calls [`messages_value`]; the text-only slot
//! rejection (`slot_err`) lives here since only this projection uses it. The
//! tool-argument string encoding is the shared `protocol::json::to_json_string`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, DocumentSource, ErrorKind, ImageSource, Role,
};
use crate::protocol::json::to_json_string;

/// Project `messages[]` (§2.2): the `system` field is prepended as one leading
/// `role:"system"` message; each `Message` then projects per its role, with a
/// `Role::Tool` fanning out to one `role:"tool"` message per `ToolResult` (§2.4).
pub(super) fn messages_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
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
            Content::ToolUse {
                id, name, input, ..
            } => calls.push(json!({
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

/// Render text/image/document parts into an OpenAI `content` value (§2.2): a single
/// `Text` part is a bare string (the common shape); otherwise the array form. A
/// text-only slot (`allow_media == false`) rejects any image/document as `ParseInput`.
fn content_value(
    parts: &[Content],
    allow_media: bool,
    slot: &str,
) -> Result<Value, CanonicalError> {
    if let [Content::Text(t)] = parts {
        return Ok(json!(t)); // bare string for a lone text part
    }
    let mut arr = Vec::new();
    for p in parts {
        match p {
            Content::Text(t) => arr.push(json!({"type": "text", "text": t})),
            Content::Image { source } if allow_media => {
                arr.push(json!({"type": "image_url", "image_url": image_url(source)}))
            }
            Content::Document { source } if allow_media => arr.push(document_file(source)?),
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

/// `Document{Base64}` → an OpenAI chat `file` part (§2.2): the base64 embeds as a
/// data-URI in `file_data`, and a `filename` (which chat REQUIRES for `file_data`) is
/// synthesized from the media type. A `Document{Url}` REJECTS — chat file inputs accept
/// no external URL (unlike `image_url`, §6 CR-6). Data-URI round-trips like the image.
fn document_file(source: &DocumentSource) -> Result<Value, CanonicalError> {
    match source {
        DocumentSource::Base64 { media_type, data } => Ok(json!({
            "type": "file",
            "file": {
                "filename": doc_filename(media_type),
                "file_data": format!("data:{media_type};base64,{data}"),
            },
        })),
        DocumentSource::Url { .. } => Err(url_document_err()),
    }
}

/// A `filename` for a base64 document — chat requires one on `file_data`. Derived from
/// the media type's subtype (`application/pdf` → `document.pdf`).
fn doc_filename(media_type: &str) -> String {
    format!(
        "document.{}",
        media_type.rsplit('/').next().unwrap_or("bin")
    )
}

/// A `Document{Url}` rejected on chat (§2.2/§6 CR-6): Chat Completions file inputs accept
/// only base64 `file_data` or an uploaded `file_id`, never a web URL. Message names the
/// remedy (Responses API, or send as a base64 document); no accepted URL form.
fn url_document_err() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: "openai chat: document URLs are not supported — Chat Completions file inputs \
                  accept only base64 file_data or an uploaded file_id, not a web URL; use the \
                  Responses API or send the document as a base64 document"
            .to_string(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// A text-only wire slot (system, tool message) rejected non-text content (§2.3/§2.4).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
