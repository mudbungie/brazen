//! The `google_generative_ai` Protocol impl (providers §4). `encode` projects a
//! `CanonicalRequest` onto `POST {base_url}/v1beta/models/{model}:streamGenerateContent`
//! (the model rides the URL path; roles are `user`/`model`); `decode` is a pure
//! `(frame, &mut DecodeState)` state machine over the SSE stream of
//! `GenerateContentResponse` chunks. There is no block-start/stop on the wire, so
//! `MessageStart`/`ContentStart` are synthesized and the **last chunk's non-null
//! `finishReason`** is the native terminator. The `x-goog-api-key` auth header is
//! pure row DATA read by the shared `ApiKeyAuth` — no new `Auth` impl (§4.1). No IO,
//! no clock, no creds — `&GoogleGenAi` is `&'static dyn`.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event, Model};
use crate::protocol::json::decode_models;
use crate::protocol::{DecodeState, Frame, Framing, Protocol, ProviderCtx, WireRequest};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct GoogleGenAi;

impl Protocol for GoogleGenAi {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError> {
        encode::encode(req, ctx)
    }

    fn path(&self, ctx: &ProviderCtx) -> String {
        // `--raw` has no parsed `stream`; target the streaming endpoint (brazen's
        // native mode), the same path `encode` builds for a streaming request.
        encode::request_path(ctx, true)
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
        "/v1beta/models"
    }

    fn decode_models(&self, body: &[u8]) -> Result<Vec<Model>, CanonicalError> {
        // `models[].name`, stripping the leading `models/` so the id is usable in
        // encode's `/v1beta/models/{model}:…` path (§3.1).
        decode_models(body, "models", "name", "models/")
    }
}
