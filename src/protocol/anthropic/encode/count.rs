//! Token-count REQUEST projection (anthropic-messages §2.11): canonical →
//! `POST /v1/messages/count_tokens` body. Reuses THIS module's own message/system/tool
//! projection leaves (`super::{system_value, messages_value, tools_value,
//! tool_choice_value}`, `super::cache`) so the counted request is the §2 body MINUS the
//! generation-only keys the count endpoint rejects (`max_tokens`, `stream`,
//! `temperature`, `top_p`, `stop_sequences`) — it does NOT call `encode` and re-strip a
//! serialized body, so `encode`'s own bytes stay untouched.

use serde_json::{json, Map};

use crate::canonical::{CanonicalError, CanonicalRequest};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

/// The count path appended to `base_url` (§2.11) — `/v1/messages` plus `/count_tokens`.
const COUNT_PATH: &str = "/v1/messages/count_tokens";

/// Build the count body (§2.11): `model`, the §6 reasoning→`thinking` object, `system`,
/// `messages`, `tools`, `tool_choice`, automatic `cache_control` marks (fidelity — the
/// count reflects the exact cached prefix), and the `extra` passthrough — but NONE of
/// the generation-only keys. The endpoint validates a `MessageCountTokensParams`, which
/// accepts exactly this subset; the omitted five do not affect the input-token count.
pub(crate) fn count(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut body = Map::new();
    body.insert("model".into(), json!(ctx.model));
    // reasoning → extended thinking (providers §6). count_tokens accepts `thinking`, and
    // it changes the count, so mirror `encode`'s object — but WITHOUT the max_tokens
    // flooring (`max_tokens` is a generation-only key, absent here).
    if let Some(effort) = req.reasoning {
        body.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": effort.budget()}),
        );
    }
    // `req.system` AND every mid-transcript `Role::System` message hoist to the ONE
    // top-level `system` array (§2.3/§2.4), the SAME projection `encode` uses; `None`
    // when there is no system content at all → omit the field.
    if let Some(system) = super::system_value(req)? {
        body.insert("system".into(), system);
    }
    body.insert("messages".into(), super::messages_value(&req.messages)?);
    if !req.tools.is_empty() {
        body.insert("tools".into(), super::tools_value(&req.tools));
    }
    if let Some(mut tc) = super::tool_choice_value(&req.tool_choice, req.tools.is_empty()) {
        if req.parallel_tool_calls == Some(false) {
            tc["disable_parallel_tool_use"] = json!(true);
        }
        body.insert("tool_choice".into(), tc);
    }
    // Automatic prompt-cache placement (§2.10) — before the `extra` fold, exactly as
    // `encode` orders it, so the count reflects the same cached prefix.
    super::cache::apply(&mut body);
    for (k, v) in &req.extra {
        body.entry(k.clone()).or_insert_with(|| v.clone());
    }
    Ok(finish_body(body, format!("{}{COUNT_PATH}", ctx.base_url)))
}
