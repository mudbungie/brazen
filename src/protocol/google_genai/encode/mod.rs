//! REQUEST projection (providers §4.2): canonical → the `generateContent` wire.
//! The model selects the URL path (`models/{model}:…`), roles are `user`/`model`,
//! generation params nest under `generationConfig`, and `system` hoists to
//! `systemInstruction`. Pure; the `x-goog-api-key` header is set later by `Auth`.
//! The `contents[]`/`systemInstruction` projection lives in [`contents`]; this
//! module owns the body assembly, the tool declarations, and `generationConfig`.

use serde_json::{json, Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind, Tool, ToolChoice};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod contents;
mod count;

pub(super) use count::count as count_body;

/// The request path appended to `base_url` (§4.2) — the one home for the
/// `models/{model}:{verb}` shape, read by both `encode` (with the request's
/// `stream`) and the `Protocol::path` impl. Streaming is the endpoint CHOICE
/// (`:streamGenerateContent?alt=sse` vs `:generateContent`), not a body field.
pub(super) fn request_path(ctx: &ProviderCtx, stream: bool) -> String {
    let verb = if stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };
    format!("/v1beta/models/{}:{}", ctx.model, verb)
}

/// Build the wire request (§4.2).
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    // The streaming intent picks `:streamGenerateContent` vs `:generateContent` (§4.2);
    // the rest of the tail (serialize, wrap, fold beta headers) is the shared one.
    let url = format!(
        "{}{}",
        ctx.base_url,
        request_path(ctx, req.stream.unwrap_or(false))
    );
    Ok(finish_body(body_map(req)?, url))
}

/// The `generateContent` body — the `systemInstruction`/`contents`/`tools`/`toolConfig`/
/// `generationConfig`/`extra` assembly (§4.2), factored out of [`encode`] so the
/// `--count-tokens` path ([`count`](count::count)) can reuse the EXACT same projection.
/// `encode`'s byte output is unchanged (same keys, same insertion order); the `model`
/// (which the URL path carries for `encode`) is NOT here — the count envelope injects it.
fn body_map(req: &CanonicalRequest) -> Result<Map<String, Value>, CanonicalError> {
    let mut body = Map::new();
    if let Some(si) = contents::system_instruction(req)? {
        body.insert("systemInstruction".into(), si);
    }
    body.insert("contents".into(), contents::contents_value(req)?);
    if !req.tools.is_empty() {
        body.insert(
            "tools".into(),
            json!([{ "functionDeclarations": fn_decls(&req.tools)? }]),
        );
    }
    if let Some(tc) = tool_config(&req.tool_choice) {
        body.insert("toolConfig".into(), tc);
    }
    let gen = generation_config(req);
    if !gen.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen));
    }
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone()); // typed fields win (§4.2)
    }
    Ok(body)
}

/// `tools[]` → `functionDeclarations` (§4.2); `description` omitted when `None`,
/// `parameters` ← `input_schema` verbatim. A provider-typed tool has no Google
/// projection — fail fast with `ParseInput` (exit 64), never a silent drop.
fn fn_decls(tools: &[Tool]) -> Result<Value, CanonicalError> {
    let mut decls = Vec::new();
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
        let mut d = json!({ "name": name, "parameters": input_schema });
        if let Some(desc) = description {
            d["description"] = json!(desc);
        }
        decls.push(d);
    }
    Ok(Value::Array(decls))
}

/// `tool_choice` → `toolConfig.functionCallingConfig` (§4.2): `Auto` omits (the
/// default `AUTO`); `Any`→`ANY`; `None`→`NONE`; `Tool{name}`→`ANY` + allow-list.
fn tool_config(tc: &ToolChoice) -> Option<Value> {
    let cfg = match tc {
        ToolChoice::Auto => return None,
        ToolChoice::Any => json!({ "mode": "ANY" }),
        ToolChoice::None => json!({ "mode": "NONE" }),
        ToolChoice::Tool { name } => {
            json!({ "mode": "ANY", "allowedFunctionNames": [name] })
        }
    };
    Some(json!({ "functionCallingConfig": cfg }))
}

/// Generation params → the nested `generationConfig` (§4.2), each omitted when
/// absent so an empty config is dropped entirely.
fn generation_config(req: &CanonicalRequest) -> Map<String, Value> {
    let mut gen = Map::new();
    if let Some(n) = req.max_tokens {
        gen.insert("maxOutputTokens".into(), json!(n));
    }
    if let Some(t) = req.temperature {
        gen.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        gen.insert("topP".into(), json!(p));
    }
    if let Some(r) = req.reasoning {
        // effort → thinkingConfig via the shared budget table (providers §6).
        gen.insert(
            "thinkingConfig".into(),
            json!({"thinkingBudget": r.budget(), "includeThoughts": true}),
        );
    }
    if !req.stop.is_empty() {
        gen.insert("stopSequences".into(), json!(req.stop)); // RENAME + nesting
    }
    gen
}
