//! The ingress edge (ingress.md): brazen accepting a request in a CLIENT dialect,
//! decoded to the canonical model at the input boundary — the mirror of the egress
//! `Protocol` adapters. An ingress dialect is a codec pair (ingress.md §2); this
//! module holds the request half (`decode_request`) plus the dialect dispatch,
//! mirroring the egress registry pattern (arch §4.4): a TOTAL match over the closed
//! [`IngressId`] enum, so adding a dialect fails to compile until its arm exists and
//! an "unregistered dialect" is unrepresentable. The response half
//! (`encode_response`, ingress.md §2) and the replay-stash re-injection (§5) are
//! sibling capabilities that join here later; nothing in this module does IO.

mod openai_chat;

use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind};

/// The closed set of ingress dialects — the registry key, mirroring `ProtocolId`
/// (arch §4.4). A dialect is always named EXPLICITLY (`[ingress].dialect`, `--in`);
/// structural sniffing stays forbidden (ingress.md §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IngressId {
    /// OpenAI `chat/completions` — wave 1, the lingua franca (ingress.md §12).
    OpenAiChat,
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
    }
}
