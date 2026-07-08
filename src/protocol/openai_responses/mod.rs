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
use crate::protocol::{
    DecodeState, Frame, Framing, ModelKeys, ModelsShape, Protocol, ProviderCtx, WireRequest,
};

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

    fn path(&self, _ctx: &ProviderCtx) -> String {
        encode::REQUEST_PATH.to_string()
    }

    fn content_type(&self) -> &str {
        "application/json"
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

    fn models_shape(&self) -> ModelsShape {
        // The PROTOCOL DEFAULT — standard OpenAI `data[].id`, no metadata keys. The
        // ChatGPT-SSO Codex row pins `[provider.models]` to override the path/query/keys
        // to its `{"models":[{"slug":…,"context_window":…}]}` shape (model-discovery §3.1,
        // §3.2) — and MAY name `context_key = "context_window"` to lift that metadata;
        // same protocol, two list shapes, the keys being ROW data not a protocol constant.
        ModelsShape {
            path: "/models",
            keys: ModelKeys {
                array_key: "data",
                id_key: "id",
                strip: "",
                context_key: "",
                max_output_key: "",
                display_name_key: "",
            },
        }
    }
}
