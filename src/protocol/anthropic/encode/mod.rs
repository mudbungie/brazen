//! REQUEST projection (anthropic-messages §2): canonical → `POST /v1/messages`
//! body + non-auth headers. Pure; the auth header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, Message, Role, Tool, ToolChoice,
};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod blocks;

/// The request path appended to `base_url` (§2.2) — the one home for `/v1/messages`,
/// read by both `encode` and the `Protocol::path` impl.
pub(super) const REQUEST_PATH: &str = "/v1/messages";

/// The answer-token allowance carved ABOVE the thinking budget when `reasoning` is set
/// (providers.md §6): Anthropic requires `max_tokens > budget_tokens` (the budget is
/// taken OUT of `max_tokens`), so the encoder floors `max_tokens` at `budget + this`,
/// guaranteeing room for both thinking and a reply. Anthropic-dialect data; the
/// effort→budget table itself lives on the shared `ReasoningEffort` (arch §3.1).
const REASONING_HEADROOM: u32 = 4096;

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
    let mut max_tokens = req.max_tokens.ok_or_else(config_err)?;
    // reasoning → extended thinking (providers.md §6). The effort→budget table is the
    // shared `ReasoningEffort::budget()`; the max_tokens coupling is Anthropic's: the
    // budget is carved OUT of max_tokens, so floor it at budget+headroom to keep
    // max_tokens > budget_tokens with room for an answer. Inserted before the `extra`
    // fold, so a typed `--reasoning` wins over a `body_defaults` `thinking` object.
    if let Some(effort) = req.reasoning {
        let budget = effort.budget();
        body.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": budget}),
        );
        max_tokens = max_tokens.max(budget + REASONING_HEADROOM);
    }
    body.insert("max_tokens".into(), json!(max_tokens));
    if let Some(system) = &req.system {
        body.insert("system".into(), system_value(system)?); // hoisted top-level
    }
    body.insert("messages".into(), messages_value(&req.messages)?);
    // Extended thinking only accepts temperature:1 and restricts top_p, so when
    // reasoning is set these sampling params are OMITTED from the wire (they'd 400);
    // they stay on the canonical request for every other protocol (providers.md §6).
    if req.reasoning.is_none() {
        if let Some(t) = req.temperature {
            body.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            body.insert("top_p".into(), json!(p));
        }
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
    // anthropic-version (and any beta) ride `ctx.beta_headers`, stamped in `serve` for
    // BOTH the encoded and `--raw` paths (bl-3e2f) — not folded in by the shared tail.
    Ok(finish_body(body, format!("{}{REQUEST_PATH}", ctx.base_url)))
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
