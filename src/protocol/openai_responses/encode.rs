//! REQUEST projection (providers §3.2): canonical → `POST {base_url}/responses`
//! body. The Responses API folds `system` into `instructions`, `messages` into a
//! typed `input[]` array, and renames `max_tokens`→`max_output_tokens`. Tools are
//! FLAT (no nested `function` envelope). Pure; the bearer header is set by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, ImageSource, Message, Role, Tool,
    ToolChoice,
};
use crate::protocol::json::{finish_body, to_json_string};
use crate::protocol::{ProviderCtx, WireRequest};

/// The request path appended to `base_url` (§3.2) — the one home for `/responses`,
/// read by both `encode` and the `Protocol::path` impl.
pub(super) const REQUEST_PATH: &str = "/responses";

/// Build the wire request (§3.2). Typed fields serialize first; `extra` folds in
/// only keys they did not set — the typed field is the single source of truth.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    if let Some(text) = instructions(req)? {
        body.insert("instructions".into(), json!(text));
    }
    body.insert("input".into(), input_value(req)?);
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools)?); // omit when empty
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice) {
        body.insert("tool_choice".into(), tc); // Auto omitted (the default)
    }
    if let Some(n) = req.max_tokens {
        body.insert("max_output_tokens".into(), json!(n)); // RENAME
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if let Some(r) = req.reasoning {
        body.insert("reasoning".into(), json!({"effort": r.as_str()})); // §reasoning (providers §6)
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false))); // usage rides response.completed
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§3.2)
    }
    Ok(finish_body(body, format!("{}{REQUEST_PATH}", ctx.base_url)))
}

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
fn instructions(req: &CanonicalRequest) -> Result<Option<String>, CanonicalError> {
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
fn input_value(req: &CanonicalRequest) -> Result<Value, CanonicalError> {
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
    let mut calls = Vec::new();
    for c in &m.content {
        match c {
            Content::Text(t) => content.push(json!({ "type": text_type, "text": t })),
            Content::Image { source } if role == "user" => content.push(input_image(source)),
            Content::ToolUse { id, name, input } => calls.push(json!({
                "type": "function_call", "call_id": id, "name": name,
                "arguments": to_json_string(input),
            })),
            Content::Thinking { .. } | Content::RedactedThinking { .. } => {} // dropped (§3.3)
            _ => return Err(slot_err(role)),
        }
    }
    if !content.is_empty() {
        items.push(json!({ "type": "message", "role": role, "content": content }));
    }
    items.extend(calls);
    Ok(())
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

/// `tools[]` → FLAT function objects (§3.2): no nested `function` envelope, unlike
/// Chat Completions. `description` omitted when `None`. A provider-typed tool is
/// not projected in this ball (Responses' NATIVE typed tools are future per-dialect
/// work, providers §9) — fail fast with `ParseInput` (exit 64), never a drop.
fn tools_value(tools: &[Tool]) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for t in tools {
        let Tool::Custom {
            name,
            description,
            input_schema,
        } = t
        else {
            return Err(CanonicalError {
                kind: ErrorKind::ParseInput,
                message: "provider-typed tools are not projected for this dialect".into(),
                provider_detail: None,
                retry_after_seconds: None,
            });
        };
        let mut f = json!({ "type": "function", "name": name, "parameters": input_schema });
        if let Some(d) = description {
            f["description"] = json!(d);
        }
        out.push(f);
    }
    Ok(Value::Array(out))
}

/// `tool_choice` spellings (§3.2): `Auto` omits (the default); `Any`→`"required"`;
/// `None`→`"none"`; `Tool{name}`→flat `{type:"function", name}`.
fn tool_choice_value(tc: &ToolChoice) -> Option<Value> {
    Some(match tc {
        ToolChoice::Auto => return None,
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({ "type": "function", "name": name }),
    })
}
