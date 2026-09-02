//! REQUEST projection (providers §3.2): canonical → `POST {base_url}/responses`
//! body. The Responses API folds `system` into `instructions`, `messages` into a
//! typed `input[]` array, and renames `max_tokens`→`max_output_tokens`. Tools are
//! FLAT (no nested `function` envelope). Pure; the bearer header is set by `Auth`.
//! The canonical-content projections (`instructions`, `input[]`) live in [`input`];
//! this module owns the top-level body assembly, the `text.format` structured-output
//! object, the `tools[]` objects and the `tool_choice` spellings — none of which read
//! a `Content`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, ErrorKind, OutputFormat, Tool, ToolChoice,
};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod input;

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
    if let Some(text) = input::instructions(req)? {
        body.insert("instructions".into(), json!(text));
    }
    body.insert("input".into(), input::input_value(req)?);
    if !req.tools.is_empty() {
        body.insert("tools".into(), tools_value(&req.tools)?); // omit when empty
    }
    if let Some(tc) = tool_choice_value(&req.tool_choice) {
        body.insert("tool_choice".into(), tc); // Auto omitted (the default)
    }
    if let Some(p) = req.parallel_tool_calls {
        body.insert("parallel_tool_calls".into(), json!(p)); // top-level, as chat (§3.2); None → omit
    }
    if let Some(n) = req.max_tokens {
        body.insert("max_output_tokens".into(), json!(n)); // RENAME
    }
    // Reasoning models (o-series/gpt-5) 400 on non-default `temperature`/`top_p` — the
    // exact models that accept `reasoning`. When reasoning is set these are OMITTED,
    // mirroring the Anthropic rule (anthropic/encode/mod.rs / providers.md §6) and the
    // openai_chat §2.7 rule; they stay on the canonical request for every other protocol.
    if req.reasoning.is_none() {
        if let Some(t) = req.temperature {
            body.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            body.insert("top_p".into(), json!(p));
        }
    }
    if let Some(r) = req.reasoning {
        // §reasoning (providers §6). BOTH channels, each serving a different consumer:
        // `summary` is the only READABLE one (Responses emits
        // `reasoning_summary_text.delta` — decoded to `ThinkingDelta`, all `--thinking`
        // renders — only when asked, bl-f90e), `encrypted_content` the opaque replay
        // state a harness feeds back statelessly (store:false — bl-61a9, §3.2).
        body.insert(
            "reasoning".into(),
            json!({"effort": r.as_str(), "summary": "auto"}),
        );
        body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
    if let Some(t) = req.service_tier {
        // §service tier (providers §6.2): the same OpenAI-family spelling as chat.
        // Before the `extra` fold, so the typed knob wins on a same-named key.
        body.insert("service_tier".into(), json!(t.openai()));
    }
    body.insert("stream".into(), json!(req.stream.unwrap_or(false))); // usage rides response.completed
    if let Some(fmt) = text_format(&req.output) {
        // §structured output: Responses nests the format under `text.format` and lays
        // `{type,name,schema,strict}` FLAT (no `json_schema` wrapper, unlike chat §2.5.1).
        body.insert("text".into(), json!({ "format": fmt }));
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§3.2)
    }
    Ok(finish_body(body, format!("{}{REQUEST_PATH}", ctx.base_url)))
}

/// `text.format` (§3.2): the portable `output` knob → Responses' structured-output
/// spelling. `Json` is JSON mode; `JsonSchema` lays `{type,name,schema,strict}` FLAT
/// (no `json_schema` wrapper). `name` defaults to `"response"`; `strict`/`None` omit.
/// `None` → no key. Caller wraps the returned object under `text.format`.
fn text_format(output: &Option<OutputFormat>) -> Option<Value> {
    Some(match output.as_ref()? {
        OutputFormat::Json => json!({ "type": "json_object" }),
        OutputFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut f = json!({
                "type": "json_schema",
                "name": name.as_deref().unwrap_or("response"),
                "schema": schema,
            });
            if let Some(s) = strict {
                f["strict"] = json!(s);
            }
            f
        }
    })
}

/// `tools[]` → FLAT function objects (§3.2): no nested `function` envelope, unlike
/// Chat Completions. `description` omitted when `None`; `strict` (the per-tool
/// structured-output knob) folds FLAT onto the tool when set. A provider-typed tool
/// is not projected in this ball (Responses' NATIVE typed tools are future per-dialect
/// work, providers §9) — fail fast with `ParseInput` (exit 64), never a drop.
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
        if let Some(s) = strict {
            f["strict"] = json!(s);
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
