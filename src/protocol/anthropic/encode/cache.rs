//! Automatic prompt-cache placement (anthropic-messages §2.10): caching is
//! brazen-owned POLICY with zero canonical surface — no request field, no flag,
//! no config key. The marks are computed from the already-built `body` arrays
//! (the SSOT for projection — the System hoist and block drops are already
//! applied), and the caller observes the outcome only through the response-side
//! `Usage.cache_read_tokens`/`cache_write_tokens`. At most 3 marks by
//! construction (head + intermediate + rolling), so the provider's 4-marker cap
//! is unreachable and no error path exists here at all.

use serde_json::{json, Map, Value};

/// Anthropic finds a cache hit only within the last ~20 content blocks before a
/// breakpoint. The intermediate mark rides this far behind the rolling mark so
/// the PREVIOUS turn's write point stays inside the lookback even when a turn's
/// delta is large.
const LOOKBACK: usize = 20;

/// Place the policy marks. Runs after the typed fields are inserted and BEFORE
/// the `extra` fold, so a policy marker wins over any raw `cache_control` an
/// `extra` key carries (§2.1.1). Sub-minimum prefixes (1024/4096 tokens by
/// model) are Anthropic's documented silent no-op — never brazen's to police,
/// so the head mark is unconditional.
pub(super) fn apply(body: &mut Map<String, Value>) {
    // Head mark — always: the last `system` block (caching the tools+system
    // prefix), else the last `tools` object, else nothing (the empty-set rule).
    let head = if has_blocks(body, "system") {
        "system"
    } else {
        "tools"
    };
    if let Some(block) = last_of(body, head) {
        mark(block);
    }
    // Conversation marks, computed from the wire `messages` array itself.
    let msgs = body.get("messages").and_then(Value::as_array);
    let targets = conversation_marks(msgs.map_or(&[] as &[_], Vec::as_slice));
    for (m, b) in targets {
        mark(&mut body["messages"][m]["content"][b]);
    }
}

/// The rolling mark (the cache cut that advances with the transcript) and, on a
/// long span, one intermediate mark [`LOOKBACK`] eligible blocks behind it —
/// wire `(message, block)` positions. Empty when the request is not an ongoing
/// conversation or nothing is eligible.
fn conversation_marks(msgs: &[Value]) -> Vec<(usize, usize)> {
    // Trigger: an assistant turn STRICTLY BEFORE the last wire message. A lone
    // trailing assistant is a prefill one-shot — the completion EXTENDS its
    // blocks, so they mutate and must not anchor a cache.
    let ongoing = msgs
        .split_last()
        .is_some_and(|(_, before)| before.iter().any(|m| m["role"] == "assistant"));
    if !ongoing {
        return Vec::new();
    }
    // The rolling mark anchors the LAST NON-ASSISTANT message (the same prefill
    // rule at the tail); earlier assistant turns are stable history, so the
    // walk-back below crosses them freely.
    let Some(last) = msgs.iter().rposition(|m| m["role"] != "assistant") else {
        return Vec::new();
    };
    // Every eligible block up to the end of that message, in wire order. The
    // final entry IS the rolling mark — stepping back past an ineligible tail
    // (§2.10 eligibility) or a 0-eligible message is inherent to "last".
    let eligible: Vec<(usize, usize)> = msgs[..=last]
        .iter()
        .enumerate()
        .flat_map(|(m, msg)| {
            msg["content"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, block)| cacheable(block))
                .map(move |(b, _)| (m, b))
        })
        .collect();
    let Some((&rolling, before)) = eligible.split_last() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if before.len() >= LOOKBACK {
        out.push(before[before.len() - LOOKBACK]);
    }
    out.push(rolling);
    out
}

/// `cache_control` is invalid on `thinking`/`redacted_thinking` blocks — a
/// natural target of that kind steps the mark back to the previous eligible one.
fn cacheable(block: &Value) -> bool {
    block["type"] != "thinking" && block["type"] != "redacted_thinking"
}

/// Does `body[key]` hold at least one block? Absent and empty collapse to one
/// "nothing there" answer (the empty-set rule).
fn has_blocks(body: &Map<String, Value>, key: &str) -> bool {
    body.get(key)
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
}

/// Last element of `body[key]` as a mutable block, or None when the key is
/// absent or empty (the two "nothing to mark" cases collapse to one).
fn last_of<'a>(body: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Value> {
    body.get_mut(key)?.as_array_mut()?.last_mut()
}

/// The ONE marker this policy ever writes. TTL is always omitted: the 5-minute
/// default renews on every cache read, so a steady loop stays warm indefinitely;
/// `1h` only wins across idle gaps a stateless adapter cannot see.
fn mark(block: &mut Value) {
    block["cache_control"] = json!({"type": "ephemeral"});
}
