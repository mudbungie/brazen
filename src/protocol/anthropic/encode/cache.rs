//! Prompt-cache breakpoint projection (anthropic-messages §2.10): canonical
//! `req.cache` → per-block `cache_control` markers on the LAST wire block of each
//! anchored region. Reads the already-built `body` arrays (SSOT for projection);
//! recomputes ONLY the System-hoist skip count to map a canonical message index to
//! its wire position. ≤4 breakpoints and resolve-or-ParseInput are enforced here.

use serde_json::{json, Map, Value};

use crate::canonical::{CacheAnchor, CacheTtl, CanonicalError, CanonicalRequest, ErrorKind, Role};

const MAX_BREAKPOINTS: usize = 4;

/// Project every breakpoint to a `cache_control` marker on the block its anchor
/// resolves to. Empty `cache` is the general path with empty input — no marker,
/// `body` byte-identical. >4 breakpoints or an anchor resolving to no wire block
/// is `ParseInput` (exit 64). Runs after the typed fields are inserted and BEFORE
/// the `extra` fold, so the projection is typed and `extra` cannot clobber it.
pub(super) fn apply(
    body: &mut Map<String, Value>,
    req: &CanonicalRequest,
) -> Result<(), CanonicalError> {
    if req.cache.is_empty() {
        return Ok(()); // general path, empty input — no marker, body unchanged
    }
    if req.cache.len() > MAX_BREAKPOINTS {
        return Err(parse_err(
            "anthropic_messages allows at most 4 cache breakpoints",
        ));
    }
    for bp in &req.cache {
        let block = resolve(body, req, &bp.anchor)?;
        // FiveMin (default) is emitted by OMITTING ttl; OneHour emits "1h".
        block["cache_control"] = match bp.ttl {
            CacheTtl::FiveMin => json!({"type": "ephemeral"}),
            CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        };
    }
    Ok(())
}

/// Resolve an anchor to the LAST wire block it marks, or ParseInput if it resolves
/// to nothing (empty tools/system, out-of-range / hoisted-System / 0-block message).
fn resolve<'a>(
    body: &'a mut Map<String, Value>,
    req: &CanonicalRequest,
    anchor: &CacheAnchor,
) -> Result<&'a mut Value, CanonicalError> {
    match anchor {
        CacheAnchor::Tools => last_of(body, "tools")
            .ok_or_else(|| parse_err("cache anchor `tools` resolves to no tool block")),
        CacheAnchor::System => last_of(body, "system")
            .ok_or_else(|| parse_err("cache anchor `system` resolves to no system block")),
        CacheAnchor::Message { index } => {
            let i = *index as usize;
            let m = req
                .messages
                .get(i)
                .ok_or_else(|| parse_err("cache anchor `message` index out of range"))?;
            if m.role == Role::System {
                return Err(parse_err(
                    "cache anchor `message` targets a hoisted system message",
                ));
            }
            // messages_value pushes ONE wire entry per non-System message, in order,
            // even when it projects to 0 blocks — so the wire position is the count of
            // non-System messages before `i` (the same `continue` skip as encode).
            let wire_pos = req.messages[..i]
                .iter()
                .filter(|m| m.role != Role::System)
                .count();
            body["messages"][wire_pos]["content"]
                .as_array_mut()
                .and_then(|a| a.last_mut())
                .ok_or_else(|| parse_err("cache anchor `message` projects to no wire block"))
        }
    }
}

/// Last element of `body[key]` as a mutable block, or None if the key is absent or
/// the array is empty (the two "nothing to mark" cases collapse to one).
fn last_of<'a>(body: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Value> {
    body.get_mut(key)?.as_array_mut()?.last_mut()
}

fn parse_err(msg: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: msg.into(),
        provider_detail: None,
    }
}
