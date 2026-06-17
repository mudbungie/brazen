//! The `openai_chat` Protocol impl (openai-chat-mapping spec). `encode` projects a
//! `CanonicalRequest` onto `POST {base_url}/chat/completions` (body + non-auth
//! headers); `decode` is a pure `(frame, &mut DecodeState)` state machine over the
//! Chat Completions SSE stream — positional `choices[0].delta`, synthesized
//! `MessageStart`/`ContentStart`, `arguments`→`JsonDelta`, the `[DONE]` terminator,
//! and the non-2xx whole-body error envelope. No IO, no clock, no creds — all
//! cross-frame state lives in the caller-owned `DecodeState`, so `&OpenAiChat` is
//! shareable as `&'static dyn Protocol`. Vendor-blind: the Chat Completions dialect
//! is shared verbatim by OpenAI, Mistral, local Ollama-in-OpenAI-mode (data rows),
//! and nothing here branches on which one sent the bytes.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::{DecodeState, Frame, Framing, Protocol, ProviderCtx, WireRequest};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct OpenAiChat;

impl Protocol for OpenAiChat {
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
