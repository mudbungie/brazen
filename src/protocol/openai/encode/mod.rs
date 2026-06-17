//! REQUEST projection (openai-chat-mapping §2): canonical → `POST
//! {base_url}/chat/completions` body + non-auth headers. Pure; the
//! `Authorization: Bearer` header is set later by `Auth`. The `messages[]`
//! projection lives in [`messages`]; this module owns the top-level body assembly,
//! the `tools[]` objects, and the `tool_choice` spellings.

use serde_json::{json, Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, Tool, ToolChoice};
use crate::protocol::{ProviderCtx, WireRequest};

mod messages;

/// Build the wire request (§2.1). Typed fields serialize first; `extra` folds in
/// only keys they did not set — the typed field is the single source of truth.
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    body.insert("messages".into(), messages::messages_value(req)?);
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools)); // omit when empty
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice) {
        body.insert("tool_choice".into(), tc); // Auto omitted (OpenAI default)
    }
    if let Some(p) = req.parallel_tool_calls {
        body.insert("parallel_tool_calls".into(), json!(p)); // top-level (§2.6); None → omit
    }
    if let Some(n) = req.max_tokens {
        body.insert("max_tokens".into(), json!(n)); // None → omit (row requires none)
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        body.insert("stop".into(), json!(req.stop)); // array form; omit when empty
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false)));
    if req.stream.unwrap_or(false) {
        // Without include_usage a streamed response carries ZERO usage (§2.8).
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§2.1.1)
    }
    // Our own owned Map of Values serializes infallibly (mirrors the Anthropic encode).
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(format!("{}/chat/completions", ctx.base_url), bytes);
    wire.set_header("content-type", "application/json");
    // Built-in OpenAI row defines no beta headers; a Mistral-style row may — ride
    // ctx.beta_headers verbatim, never hard-coded, never branched on a vendor name.
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    Ok(wire)
}

/// `tools[]` → nested function objects (§2.5); `description` omitted when `None`,
/// `parameters` carries the schema verbatim.
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

/// `tool_choice` spellings (§2.6): `Auto` is omitted (OpenAI's own default); the
/// rest emit explicit values — note `Any` → `"required"`.
fn tool_choice_value(tc: &ToolChoice) -> Option<Value> {
    Some(match tc {
        ToolChoice::Auto => return None,
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({"type": "function", "function": {"name": name}}),
    })
}
