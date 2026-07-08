//! The per-content-block projection of the Anthropic request (§2.5): one canonical
//! `Content` → one wire ContentBlockParam, the image-source shape, and the
//! text/image-only `tool_result.content` slot. `super::encode` projects each
//! message's content through [`content_block`]; the shared `slot_err` is the parent's.

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, Content, ImageSource};

use super::slot_err;

/// One `Content` → one wire ContentBlockParam (§2.5). `Ok(None)` drops a block
/// that cannot be replayed (a signature-less `Thinking`, CR-2). Server-tool blocks
/// pass through VERBATIM — never folded into `tool_use`/`tool_result` (converting
/// them makes the API demand a nonexistent client `tool_result` and 400).
pub(super) fn content_block(c: &Content) -> Result<Option<Value>, CanonicalError> {
    Ok(Some(match c {
        Content::Text(t) => json!({"type": "text", "text": t}),
        Content::Image { source } => json!({"type": "image", "source": image_source(source)}),
        // `signature` (Google thoughtSignature) is ignored: Anthropic tool_use has none.
        Content::ToolUse {
            id, name, input, ..
        } => {
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
        // Anthropic replays thinking text + signature; the Responses id/encrypted_content
        // fields are ignored here (not an Anthropic concept).
        Content::Thinking {
            text,
            signature: Some(sig),
            ..
        } => json!({"type": "thinking", "thinking": text, "signature": sig}),
        // A signature-less thinking block cannot be replayed to Anthropic (the API 400s
        // on an absent signature) — dropped (CR-2, kept under bl-61a9).
        Content::Thinking {
            signature: None, ..
        } => return Ok(None),
        Content::RedactedThinking { data } => json!({"type": "redacted_thinking", "data": data}),
        Content::ServerToolUse { id, name, input } => {
            json!({"type": "server_tool_use", "id": id, "name": name, "input": input})
        }
        Content::ServerToolResult {
            kind,
            tool_use_id,
            content,
        } => json!({"type": kind, "tool_use_id": tool_use_id, "content": content}),
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
