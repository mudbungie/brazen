//! REQUEST projection (openai-chat-mapping §2): canonical → `POST
//! {base_url}/chat/completions` body + non-auth headers. Pure; the
//! `Authorization: Bearer` header is set later by `Auth`. The `messages[]`
//! projection lives in [`messages`]; this module owns the top-level body assembly,
//! the `tools[]` objects, and the `tool_choice` spellings.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, ErrorKind, OutputFormat, Tool, ToolChoice,
};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod messages;

/// The request path appended to `base_url` (§2.1) — the one home for `/chat/completions`,
/// read by both `encode` and the `Protocol::path` impl.
pub(super) const REQUEST_PATH: &str = "/chat/completions";

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
        body.insert("tools".into(), tools_value(&req.tools)?); // omit when empty
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice) {
        body.insert("tool_choice".into(), tc); // Auto omitted (OpenAI default)
    }
    if let Some(p) = req.parallel_tool_calls {
        body.insert("parallel_tool_calls".into(), json!(p)); // top-level (§2.6); None → omit
    }
    // `req.reasoning` IS the reasoning-model signal (no model-name sniffing, §2.7): an
    // o-series/gpt-5 request REJECTS the deprecated `max_tokens` (wants
    // `max_completion_tokens`) and 400s on non-default `temperature`/`top_p`. So when
    // reasoning is set the key renames and sampling is omitted — the same reframe as the
    // Anthropic sampling-drop rule (anthropic/encode/mod.rs / providers.md §6).
    if let Some(n) = req.max_tokens {
        let key = if req.reasoning.is_some() {
            "max_completion_tokens" // reasoning models reject the deprecated `max_tokens`
        } else {
            "max_tokens" // None → omit (row requires none)
        };
        body.insert(key.into(), json!(n));
    }
    if req.reasoning.is_none() {
        if let Some(t) = req.temperature {
            body.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            body.insert("top_p".into(), json!(p));
        }
    }
    if let Some(r) = req.reasoning {
        body.insert("reasoning_effort".into(), json!(r.as_str())); // §reasoning (providers §6)
    }
    if !req.stop.is_empty() {
        body.insert("stop".into(), json!(req.stop)); // array form; omit when empty
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false)));
    if req.stream.unwrap_or(false) {
        // Without include_usage a streamed response carries ZERO usage (§2.8).
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if let Some(rf) = response_format(&req.output) {
        body.insert("response_format".into(), rf); // §structured output; None → omit
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§2.1.1)
    }
    // Built-in OpenAI row defines no beta headers; a Mistral-style row may — they ride
    // `ctx.beta_headers`, stamped in `serve` for both paths (bl-3e2f), never branched.
    Ok(finish_body(body, format!("{}{REQUEST_PATH}", ctx.base_url)))
}

/// `response_format` (§2.5.1): the portable `output` knob → OpenAI's structured-output
/// spelling. `Json` is JSON mode; `JsonSchema` nests `{name, schema, strict}` under
/// `json_schema` (chat's shape, unlike Responses' flat `text.format`). `name` defaults
/// to `"response"` (chat requires it); `strict`/`None` are omitted. `None` → no key.
fn response_format(output: &Option<OutputFormat>) -> Option<Value> {
    Some(match output.as_ref()? {
        OutputFormat::Json => json!({"type": "json_object"}),
        OutputFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut js = json!({"name": name.as_deref().unwrap_or("response"), "schema": schema});
            if let Some(s) = strict {
                js["strict"] = json!(s);
            }
            json!({"type": "json_schema", "json_schema": js})
        }
    })
}

/// `tools[]` → nested function objects (§2.5); `description` omitted when `None`,
/// `parameters` carries the schema verbatim, `strict` (the per-tool structured-output
/// knob) folds onto `function` when set. A provider-typed tool has no Chat Completions
/// projection — fail fast with `ParseInput` (exit 64), never a drop.
fn tools_value(tools: &[Tool]) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for t in tools {
        let Tool::Custom {
            name,
            description,
            input_schema,
            strict,
        } = t
        else {
            return Err(provider_tool_err());
        };
        let mut f = json!({"name": name, "parameters": input_schema});
        if let Some(d) = description {
            f["description"] = json!(d);
        }
        if let Some(s) = strict {
            f["strict"] = json!(s);
        }
        out.push(json!({"type": "function", "function": f}));
    }
    Ok(Value::Array(out))
}

/// A `Tool::Provider` reached a dialect with no provider-typed-tool projection
/// (openai-chat-mapping §6): reject at encode, a documented degradation.
fn provider_tool_err() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: "provider-typed tools are not projected for this dialect".into(),
        provider_detail: None,
        retry_after_seconds: None,
    }
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
