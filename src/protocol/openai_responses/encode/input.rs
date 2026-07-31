//! The canonical-content projections of a Responses request (§3.2/§3.3): `system` →
//! the top-level `instructions` string, and `messages[]` → the typed `input[]` array
//! (`message` / `reasoning` / `function_call` / `function_call_output` items, with the
//! image and document parts each role admits). This is the whole of encode that reads
//! `Content` — `super::encode` calls [`instructions`] and [`input_value`], and the
//! text-only slot rejection (`slot_err`) lives here since only these projections use
//! it. The tool-argument string encoding is the shared `protocol::json::to_json_string`.

use serde_json::{json, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, DocumentSource, ErrorKind, ImageSource, Message,
    Role,
};
use crate::protocol::json::to_json_string;

/// A text-only wire slot rejected non-text content (§3.2/§3.3).
fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// `system` → the top-level `instructions` string (§3.2): text-only, `None` when
/// empty. `Role::System` messages stay distinct in `input[]` (§3.3).
pub(super) fn instructions(req: &CanonicalRequest) -> Result<Option<String>, CanonicalError> {
    let Some(system) = req.system.as_ref().filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let mut text = String::new();
    for c in system {
        match c {
            Content::Text(t) => text.push_str(t),
            _ => return Err(slot_err("instructions")),
        }
    }
    Ok(Some(text))
}

/// Project `messages[]` to the typed `input[]` (§3.3): each message yields a
/// `message` item for its text/image parts plus standalone `function_call` /
/// `function_call_output` items for tool use/results.
pub(super) fn input_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
    let mut items = Vec::new();
    for m in &req.messages {
        message_items(m, &mut items)?;
    }
    Ok(Value::Array(items))
}

/// One `Message` → its `input[]` items (§3.3). A `Role::Tool` carries only
/// `function_call_output` items; other roles fold text/image into one `message`
/// item, hoisting `ToolUse` to standalone `function_call` items (kept in order).
fn message_items(m: &Message, items: &mut Vec<Value>) -> Result<(), CanonicalError> {
    let (role, text_type) = match m.role {
        Role::User => ("user", "input_text"),
        Role::System => ("system", "input_text"),
        Role::Assistant => ("assistant", "output_text"),
        Role::Tool => {
            for c in &m.content {
                items.push(function_call_output(c)?);
            }
            return Ok(());
        }
    };
    let mut content = Vec::new();
    let mut reasonings = Vec::new();
    let mut calls = Vec::new();
    for c in &m.content {
        match c {
            Content::Text(t) => content.push(json!({ "type": text_type, "text": t })),
            Content::Image { source } if role == "user" => content.push(input_image(source)),
            Content::Document { source } if role == "user" => content.push(input_file(source)),
            Content::ToolUse {
                id, name, input, ..
            } => calls.push(json!({
                "type": "function_call", "call_id": id, "name": name,
                "arguments": to_json_string(input),
            })),
            // A reasoning item replays ONLY when its encrypted_content is present (the
            // stateless store:false path) — a bare summary cannot be replayed (§3.3, bl-61a9).
            Content::Thinking {
                text,
                id,
                encrypted_content: Some(enc),
                ..
            } => reasonings.push(reasoning_item(text, id.as_deref(), enc)),
            Content::Thinking { .. } | Content::RedactedThinking { .. } => {} // dropped (§3.3)
            _ => return Err(slot_err(role)),
        }
    }
    items.extend(reasonings); // reasoning precedes the message/function_call it reasoned about
    if !content.is_empty() {
        items.push(json!({ "type": "message", "role": role, "content": content }));
    }
    items.extend(calls);
    Ok(())
}

/// A `reasoning` input item for stateless (`store:false`) replay (§3.3, bl-61a9): the
/// `encrypted_content` blob IS the reasoning state; the `id` (`rs_…`) is echoed when
/// present; the `summary` carries `text` when non-empty, else `[]`.
fn reasoning_item(text: &str, id: Option<&str>, encrypted_content: &str) -> Value {
    let summary = if text.is_empty() {
        json!([])
    } else {
        json!([{ "type": "summary_text", "text": text }])
    };
    let mut item = json!({
        "type": "reasoning", "summary": summary, "encrypted_content": encrypted_content,
    });
    if let Some(id) = id {
        item["id"] = json!(id);
    }
    item
}

/// `ToolResult` → a `function_call_output` item (§3.3): text-only `output`, keyed by
/// `call_id`. `is_error` surfaces textually (prefix); non-`Text` content rejects.
fn function_call_output(c: &Content) -> Result<Value, CanonicalError> {
    let Content::ToolResult {
        tool_use_id,
        content,
        is_error,
    } = c
    else {
        return Err(slot_err("tool"));
    };
    let mut text = String::new();
    for part in content {
        match part {
            Content::Text(t) => text.push_str(t),
            _ => return Err(slot_err("tool_result")),
        }
    }
    if *is_error {
        text = format!("[error] {text}");
    }
    Ok(json!({ "type": "function_call_output", "call_id": tool_use_id, "output": text }))
}

/// `Image` source → a Responses `input_image` part (§3.3): base64 embeds as a
/// data-URI (round-trips, as Chat Completions); a URL passes through.
fn input_image(source: &ImageSource) -> Value {
    let url = match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
    };
    json!({ "type": "input_image", "image_url": url })
}

/// `Document` source → a Responses `input_file` part (§3.3): base64 embeds as a data-URI
/// in `file_data` (with a `filename` synthesized from the media type, required for
/// `file_data`); a URL passes through as `file_url` — Responses fetches web URLs, so BOTH
/// sources express here (unlike Chat, which rejects the URL, §6 CR-C6).
fn input_file(source: &DocumentSource) -> Value {
    match source {
        DocumentSource::Base64 { media_type, data } => json!({
            "type": "input_file",
            "filename": format!("document.{}", media_type.rsplit('/').next().unwrap_or("bin")),
            "file_data": format!("data:{media_type};base64,{data}"),
        }),
        DocumentSource::Url { url } => json!({ "type": "input_file", "file_url": url }),
    }
}
