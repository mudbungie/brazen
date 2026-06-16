//! The `ollama_chat` Protocol impl (providers §5). `encode` projects a
//! `CanonicalRequest` onto `POST {base_url}/api/chat` (generation params nested
//! under `options`); `decode` is a pure `(frame, &mut DecodeState)` state machine
//! over the **NDJSON** stream — one JSON object per line, tool calls arriving whole,
//! `{"done":true}` the native terminator. Its distinctive cost is one DATA return:
//! `framing() -> Framing::Ndjson`, routed by `run`'s `framing.decoder()` with no new
//! framer and no branch in `run`. No IO, no clock, no creds — all cross-frame state
//! lives in the caller-owned `DecodeState`, so `&OllamaChat` is `&'static dyn`.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::{DecodeState, Frame, Framing, Protocol, ProviderCtx, WireRequest};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct OllamaChat;

impl Protocol for OllamaChat {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError> {
        encode::encode(req, ctx)
    }

    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
        decode::decode(frame, state)
    }

    /// The one mechanical difference from the SSE dialects (providers §5.2): NDJSON
    /// line framing as DATA, never behaviour.
    fn framing(&self) -> Framing {
        Framing::Ndjson
    }
}
