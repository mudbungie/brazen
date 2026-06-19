//! REQUEST projection (anthropic-messages §2): canonical → `POST /v1/messages`
//! body + non-auth headers. Pure; the auth header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, Message, Role, Tool, ToolChoice,
};
use crate::protocol::{ProviderCtx, WireRequest};

mod blocks;

/// The request path appended to `base_url` (§2.2) — the one home for `/v1/messages`,
/// read by both `encode` and the `Protocol::path` impl.
pub(super) const REQUEST_PATH: &str = "/v1/messages";

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
    body.insert("stream".into(), json!(req.stream.unwrap_or(false)));
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools));
    }
    if let Some(mut tc) = tool_choice_value(&req.tool_choice, req.tools.is_empty()) {
        // disable_parallel_tool_use lives INSIDE the tool_choice object (§2.7), so
        // the canonical knob folds here, not via the top-level `extra` valve. Only
        // Some(false) is emitted; Some(true)/None are Anthropic's default (enabled).
        if req.parallel_tool_calls == Some(false) {
            tc["disable_parallel_tool_use"] = json!(true);
        }
        body.insert("tool_choice".into(), tc);
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§2.1.1)
    }
    // Our own owned Map of Values serializes infallibly (mirrors NdjsonSink §5.2).
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(format!("{}{REQUEST_PATH}", ctx.base_url), bytes);
    // content-type rides via `Protocol::content_type()`, stamped once in `serve` for
    // BOTH this path and `--raw` (the single home for the dialect's media type).
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
pub(super) fn slot_err(slot: &str) -> CanonicalError {
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
            if let Some(b) = blocks::content_block(c)? {
                blocks.push(b);
            }
        }
        out.push(json!({"role": role, "content": Value::Array(blocks)}));
    }
    Ok(Value::Array(out))
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
