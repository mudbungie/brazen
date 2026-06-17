//! The `anthropic_messages` Protocol impl (anthropic-messages spec). `encode`
//! projects a `CanonicalRequest` onto `POST /v1/messages` (body + non-auth
//! headers); `decode` is a pure `(frame, &mut DecodeState)` state machine over the
//! SSE stream, dispatching on `data.type`. No IO, no clock, no creds — all
//! cross-frame state lives in the caller-owned `DecodeState`, so `&AnthropicMessages`
//! is shareable as `&'static dyn Protocol`. Vendor-blind: it reads only
//! `ProviderCtx`, never the string `"anthropic"` (that was spent on the lookup).
//! The two directions split across `encode`/`decode` so each file stays small.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::{DecodeState, Frame, Framing, Protocol, ProviderCtx, WireRequest};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct AnthropicMessages;

impl Protocol for AnthropicMessages {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError> {
        encode::encode(req, ctx)
    }

    fn path(&self, _ctx: &ProviderCtx) -> String {
        encode::REQUEST_PATH.to_string()
    }

    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
        decode::decode(frame, state)
    }

    fn framing(&self) -> Framing {
        Framing::Sse
    }
}
