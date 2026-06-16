//! The `openai_responses` Protocol impl (providers §3). `encode` projects a
//! `CanonicalRequest` onto `POST {base_url}/responses` — `system`→`instructions`,
//! `messages`→a typed `input[]`, `max_tokens`→`max_output_tokens`; `decode` is a
//! pure `(frame, &mut DecodeState)` state machine over the Responses SSE stream of
//! typed `response.*` events. Unlike the synthesized-structure dialects, the wire
//! carries explicit block structure, so the canonical index keys off the wire
//! `output_index` (Anthropic-style); `response.completed` is the native terminator.
//! No IO, no clock, no creds — `&OpenAiResponses` is `&'static dyn`.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::{DecodeState, Frame, Framing, Protocol, ProviderCtx, WireRequest};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct OpenAiResponses;

impl Protocol for OpenAiResponses {
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

    fn framing(&self) -> Framing {
        Framing::Sse
    }
}
