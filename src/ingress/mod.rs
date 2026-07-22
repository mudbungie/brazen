//! The ingress edge (ingress.md): brazen accepting a request in a CLIENT dialect,
//! decoded to the canonical model at the input boundary — the mirror of the egress
//! `Protocol` adapters. An ingress dialect is a codec pair (ingress.md §2); this
//! module holds both codec halves (`decode_request` / `encode_response`) plus the
//! dialect dispatch, mirroring the egress registry pattern (arch §4.4): a TOTAL
//! match over the closed [`IngressId`] enum, so adding a dialect fails to compile
//! until its arm exists and an "unregistered dialect" is unrepresentable. The
//! shared encode state lives in [`state`]; the replay-stash re-injection (§5) is a
//! sibling capability that joins here later; nothing in this module does IO.

mod anthropic_messages;
mod openai_chat;
mod reinject;
pub(crate) mod state;

pub(crate) use reinject::{reinject, THINKING_REPLAY};
pub use state::IngressState;

use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind, Event};

/// The closed set of ingress dialects — the registry key, mirroring `ProtocolId`
/// (arch §4.4). A dialect is always named EXPLICITLY (the `--in` flag, or under
/// `--serve` the route path); structural sniffing stays forbidden (ingress.md §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IngressId {
    /// OpenAI `chat/completions` — wave 1, the lingua franca (ingress.md §12).
    OpenAiChat,
    /// Anthropic `POST /v1/messages` — wave 2, the Claude ecosystem (ingress.md §12).
    AnthropicMessages,
}

/// The flag spellings of the closed dialect set — how `--in DIALECT` names an
/// [`IngressId`] (ingress.md §2, §11). `None` is the caller's error to class:
/// `--in` maps it to usage (64) — the vocabulary itself has one home. (Under
/// `--serve` the route path selects the codec, so no spelling is parsed, §8.)
pub(crate) fn dialect_id(name: &str) -> Option<IngressId> {
    match name {
        "openai_chat" => Some(IngressId::OpenAiChat),
        "anthropic_messages" => Some(IngressId::AnthropicMessages),
        _ => None,
    }
}

/// A `decode_request` failure. ALWAYS `ErrorKind::ParseInput` (ingress.md §2) — the
/// kind is a query over this type, never a stored field — framed in the CLIENT
/// dialect's error envelope at the edge (§9). `message` names the offending
/// key/shape, per the adapt-or-reject ladder's rung 4 (§3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngressError {
    pub message: String,
}

/// The one projection onto the canonical error model: an ingress rejection is
/// `ParseInput` by construction (ingress.md §2) with brazen as the origin — no
/// provider detail and no retry pacing, because no round-trip happened.
impl From<IngressError> for CanonicalError {
    fn from(e: IngressError) -> Self {
        CanonicalError {
            kind: ErrorKind::ParseInput,
            message: e.message,
            provider_detail: None,
            retry_after_seconds: None,
        }
    }
}

/// Client-dialect request bytes → the canonical request (ingress.md §2). Pure; the
/// dialect dispatches by a total match on the closed enum — the ingress mirror of
/// `Registry::protocol` (arch §4.4), never a match on a vendor name.
pub fn decode_request(dialect: IngressId, bytes: &[u8]) -> Result<CanonicalRequest, IngressError> {
    match dialect {
        IngressId::OpenAiChat => openai_chat::decode_request(bytes),
        IngressId::AnthropicMessages => anthropic_messages::decode_request(bytes),
    }
}

/// The canonical event stream → client-dialect response bytes (ingress.md §2):
/// pure, total (consumes every event including `Error`, §9), streaming-capable —
/// called once per event, emitting zero or more byte chunks (SSE frames, or the
/// `End`-rendered aggregate — the same fold on both shapes, §10). The dialect
/// dispatches by the same total match as [`decode_request`].
pub fn encode_response(dialect: IngressId, event: &Event, state: &mut IngressState) -> Vec<u8> {
    match dialect {
        IngressId::OpenAiChat => openai_chat::encode_response(event, state),
        IngressId::AnthropicMessages => anthropic_messages::encode_response(event, state),
    }
}
