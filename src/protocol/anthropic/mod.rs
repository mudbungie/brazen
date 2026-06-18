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

use crate::canonical::{CanonicalError, CanonicalRequest, Event, Model};
use crate::protocol::json::decode_models;
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

    fn decode_full(
        &self,
        body: &[u8],
        state: &mut DecodeState,
    ) -> Result<Vec<Event>, CanonicalError> {
        decode::decode_full(body, state)
    }

    fn framing(&self) -> Framing {
        Framing::Sse
    }

    fn models_path(&self) -> &str {
        "/v1/models" // base is bare (no /v1), unlike openai_chat (§3.1)
    }

    fn decode_models(&self, body: &[u8]) -> Result<Vec<Model>, CanonicalError> {
        decode_models(body, "data", "id", "") // `data[].id` (newest-first), as-is (§3.1)
    }
}
