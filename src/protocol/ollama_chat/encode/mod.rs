//! REQUEST projection (providers §5.3): canonical → `POST {base_url}/api/chat`
//! body + non-auth headers. OpenAI-chat-shaped, but generation params nest under
//! `options` and tool-call `arguments` ride as a JSON **object** (not a string).
//! Pure; the `Authorization: Bearer` header is set later by `Auth`. The
//! `messages[]` projection lives in [`messages`]; this module owns the top-level
//! body assembly, the nested `options`, and the `tools[]` objects.

use serde_json::{json, Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, Tool};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod messages;

/// The request path appended to `base_url` (§5.3) — the one home for `/api/chat`,
/// read by both `encode` and the `Protocol::path` impl.
pub(super) const REQUEST_PATH: &str = "/api/chat";

/// Build the wire request (§5.3). Typed fields serialize first; `extra` folds in
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
    let options = options_value(req);
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options)); // generation params nest here
    }
    if req.reasoning.is_some() {
        // Ollama's `think` is a top-level bool with NO effort granularity, so any
        // effort collapses to ON (providers §6). A non-reasoning model opts out via
        // `unsupported_body_keys = ["reasoning"]` (config §4.1.1).
        body.insert("think".into(), json!(true));
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false)));
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§5.3)
    }
    Ok(finish_body(body, format!("{}{REQUEST_PATH}", ctx.base_url)))
}

/// `max_tokens`/`temperature`/`top_p`/`stop` → the nested `options` map (§5.3),
/// each omitted when absent so an empty `options` is dropped entirely.
fn options_value(req: &CanonicalRequest) -> Map<String, Value> {
    let mut options = Map::new();
    if let Some(n) = req.max_tokens {
        options.insert("num_predict".into(), json!(n)); // RENAME
    }
    if let Some(t) = req.temperature {
        options.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        options.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        options.insert("stop".into(), json!(req.stop));
    }
    options
}

/// `tools[]` → OpenAI-chat-shaped function objects (§5.3); `description` omitted
/// when `None`, `parameters` carries the schema verbatim.
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
