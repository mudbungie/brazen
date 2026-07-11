//! The decode-side replay-stash join (ingress.md §5): assistant turns in the
//! inbound client transcript are joined back to their stashed opaque payloads
//! and the canonical request is recomposed with them in place — the thinking
//! block re-injected BEFORE its tool call, exactly as the upstream requires;
//! a stashed tool-call `signature` (Google `thoughtSignature`) restored onto
//! the decoded `ToolUse` it belongs to. The join key is what the client
//! provably echoes: EVERY tool-call id of a tool-bearing turn, else the shared
//! [`content_key`] hash of the turn's text. A miss fails open (§5): the turn
//! proceeds un-reasoned — but a miss on a turn the upstream would REQUIRE the
//! payload for (a tool continuation on a reasoning-enabled request, the
//! Anthropic 400 case) is the rung-3 lossy adaptation [`THINKING_REPLAY`]:
//! adapt-by-default (record the name for the §4 runtime exposure), or collapse
//! to rung 4 when the knob says `reject`.

use crate::canonical::{CanonicalRequest, Content, Message, Role};
use crate::store::{content_key, ReplayStash};

use super::IngressError;

/// The one lossy-adaptation name this module can fire (ingress.md §4, §5) —
/// the `lossy_overrides` key and the runtime-exposure spelling.
pub(crate) const THINKING_REPLAY: &str = "thinking_replay";

/// Recall + re-inject every assistant turn's stashed payload (ingress.md §5).
/// Returns the fired lossy-adaptation names (deduplicated — the exposure names
/// the adaptation, not each turn it degraded); `reject` collapses the miss to
/// the rung-4 refusal instead (`lossy_overrides = { thinking_replay = "reject" }`).
pub(crate) fn reinject(
    req: &mut CanonicalRequest,
    stash: &ReplayStash,
    reject: bool,
) -> Result<Vec<String>, IngressError> {
    // The "would need the payload" predicate (§5): the upstream requires the
    // thinking block only on a tool continuation of a REASONING turn — and the
    // client's own reasoning knob is the one honest signal this edge has. A
    // plain tool conversation (no reasoning asked) legitimately has no stash
    // entry and no requirement, so its misses stay silent.
    let reasoning = req.reasoning.is_some();
    let mut fired = false;
    for msg in req.messages.iter_mut() {
        if msg.role != Role::Assistant {
            continue;
        }
        let ids = tool_ids(msg);
        match recall(stash, &ids, msg) {
            Some(blocks) => inject(msg, blocks),
            None if !ids.is_empty() && reasoning => {
                if reject {
                    return Err(IngressError {
                        message: format!(
                            "replay stash miss for tool call `{}` on a reasoning continuation, \
                             and `lossy_overrides.thinking_replay = \"reject\"` refuses the \
                             degraded turn — drop the override to adapt, or start a fresh turn",
                            ids[0]
                        ),
                    });
                }
                fired = true;
            }
            None => {}
        }
    }
    Ok(if fired {
        vec![THINKING_REPLAY.to_owned()]
    } else {
        Vec::new()
    })
}

/// The turn's tool-call ids, in wire order — the tool-bearing join keys (§5).
fn tool_ids(msg: &Message) -> Vec<String> {
    msg.content
        .iter()
        .filter_map(|c| match c {
            Content::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The stashed payload blocks for this turn: any echoed tool-call id recalls
/// (the encoder stashed the whole payload under every id), else the content
/// hash of the turn's text. A missing entry — or one that no longer parses as
/// canonical blocks — is `None`, the §5 fail-open path, never an error.
fn recall(stash: &ReplayStash, ids: &[String], msg: &Message) -> Option<Vec<Content>> {
    let payload = if ids.is_empty() {
        stash.recall(&content_key(&turn_text(msg)))
    } else {
        ids.iter().find_map(|id| stash.recall(id))
    };
    serde_json::from_slice(&payload?).ok()
}

/// The turn's text, concatenated exactly as the encoder accumulated it into
/// the stash key (`IngressState.text` — every text delta, no separator), so
/// both directions derive one key from one fact.
fn turn_text(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// Recompose the turn with its payload blocks in place (§5): thinking /
/// redacted-thinking blocks lead the turn's content in stash (wire) order —
/// before the text and before the tool calls, where the upstream requires
/// them — and a stashed tool-call `signature` is restored onto the decoded
/// `ToolUse` bearing the same id (the Google replay fact). Any other stashed
/// block kind has no re-injection slot and is dropped (fail-open).
fn inject(msg: &mut Message, blocks: Vec<Content>) {
    let mut lead = Vec::new();
    for block in blocks {
        match block {
            Content::Thinking { .. } | Content::RedactedThinking { .. } => lead.push(block),
            Content::ToolUse {
                id,
                signature: Some(sig),
                ..
            } => {
                for c in &mut msg.content {
                    if let Content::ToolUse {
                        id: cid, signature, ..
                    } = c
                    {
                        if *cid == id {
                            *signature = Some(sig.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    msg.content.splice(0..0, lead);
}
