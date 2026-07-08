//! REQUEST projection (anthropic-messages §2): canonical → `POST /v1/messages`
//! body + non-auth headers. Pure; the auth header is set later by `Auth`.

use serde_json::{json, Map, Value};

use crate::canonical::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, Message, Role, Tool, ToolChoice,
};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

mod blocks;
mod cache;
mod count;

pub(super) use count::count as count_body;

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
    // `req.system` AND every mid-transcript `Role::System` message hoist to the ONE
    // top-level `system` array (§2.3/§2.4; architecture.md §3.1: "Anthropic hoists
    // either to its top-level `system`"). `None` when there is no system content at
    // all → omit the field (the empty-set path §2.10's head cache mark reads).
    if let Some(system) = system_value(req)? {
        body.insert("system".into(), system);
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
    // Automatic prompt-cache placement (§2.10): policy `cache_control` marks are
    // computed from the request's own shape on the already-built tools/system/
    // messages arrays. Before the `extra` fold so a policy marker wins over any
    // raw `cache_control` an `extra` key carries.
    cache::apply(&mut body);
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
        retry_after_seconds: None,
    }
}

/// A text-only / text-image-only wire slot rejected non-representable content (§2.4/§2.5).
pub(super) fn slot_err(slot: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("{slot} accepts only text content"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// The top-level `system` array (§2.3/§2.4): `req.system` blocks FIRST, then each
/// mid-transcript `Role::System` message's blocks in transcript order — BOTH are
/// hoisted here, never written inline into `messages[]` (architecture.md §3.1:
/// `req.system` and `Role::System` are two distinct facts sharing one wire home on
/// Anthropic). Always the array form (wire-equivalent to the bare string, never
/// loses caching). `None` when there is no system content at all — omit the field.
fn system_value(req: &CanonicalRequest) -> Result<Option<Value>, CanonicalError> {
    let mut blocks = Vec::new();
    if let Some(system) = &req.system {
        push_text_blocks(system, &mut blocks)?;
    }
    for m in &req.messages {
        if m.role == Role::System {
            push_text_blocks(&m.content, &mut blocks)?;
        }
    }
    Ok((!blocks.is_empty()).then_some(Value::Array(blocks)))
}

/// Append `content` to the text-only `system` slot: each block MUST be `Text`, else
/// the slot cannot express it → reject `Error{ParseInput}` (→ exit 64) (§2.4). The
/// one rule shared by both hoist sources (`req.system` and a `Role::System` message).
fn push_text_blocks(content: &[Content], out: &mut Vec<Value>) -> Result<(), CanonicalError> {
    for c in content {
        match c {
            Content::Text(t) => out.push(json!({"type": "text", "text": t})),
            _ => return Err(slot_err("system")),
        }
    }
    Ok(())
}

/// Project `messages[]` (§2.3): `System` is hoisted to top-level `system` (never
/// inline); `Tool` becomes a `"user"` message carrying `tool_result` blocks.
fn messages_value(msgs: &[Message]) -> Result<Value, CanonicalError> {
    let mut out = Vec::new();
    for m in msgs {
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue, // hoisted to `system` by system_value (§2.3)
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

/// `tools[]` (§2.6): a `Custom` tool is the flat custom-tool object (`description`
/// omitted when `None`); a `Provider` tool re-emits its opaque `kind` as the wire
/// `type` plus every config key verbatim — no `input_schema`, no `description`.
fn tools_value(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| match t {
                Tool::Custom {
                    name,
                    description,
                    input_schema,
                } => {
                    let mut o = json!({"name": name, "input_schema": input_schema});
                    if let Some(d) = description {
                        o["description"] = json!(d);
                    }
                    o
                }
                Tool::Provider { kind, name, config } => {
                    let mut o = json!({"type": kind, "name": name});
                    for (k, v) in config {
                        o[k] = v.clone();
                    }
                    o
                }
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
