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
use crate::protocol::{
    CountRequest, DecodeState, Frame, Framing, ModelKeys, ModelsShape, Protocol, ProviderCtx,
    WireRequest,
};

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
        // `data[].id` (newest-first), as-is; base is bare (no /v1) so the path
        // carries it, unlike openai_chat (§3.1). Anthropic's list serves `display_name`
        // (and `created_at`, unlifted) but NO token limits, so only the label is carried
        // — the rest stay `None` (§3, empty-set rule).
        ModelsShape {
            path: "/v1/models",
            keys: ModelKeys {
                array_key: "data",
                id_key: "id",
                strip: "",
                context_key: "",
                max_output_key: "",
                display_name_key: "display_name",
            },
        }
    }

    fn count_tokens(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Option<Result<CountRequest, CanonicalError>> {
        // `POST /v1/messages/count_tokens`, response `{"input_tokens": N}` (§2.11).
        Some(encode::count_body(req, ctx).map(|wire| CountRequest {
            wire,
            token_key: "input_tokens",
        }))
    }
}
