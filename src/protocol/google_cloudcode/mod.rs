//! The `google_cloudcode` Protocol impl (providers §4.10): the `google_genai` wire in
//! the two-key envelope of Google's Cloud Code backend — the backend the Antigravity
//! client speaks, and where an individual Google sign-in is served now that the
//! Gemini API's own OAuth is closed to individuals (auth §11). `encode` wraps §4.2's
//! body as `{"model", "request"}` on `POST {base_url}/v1internal:streamGenerateContent`
//! (no model segment in the path); `decode` unwraps `{"response": <chunk>}` and hands
//! the chunk to §4.4's fold. Nothing Google-shaped is restated here: the body assembly
//! and the chunk fold are `google_genai`'s own, reached through `pub(crate)`. The
//! entitlement gate is a `User-Agent` on the OAuth row (auth §11.1), not code. No IO,
//! no clock, no creds — `&GoogleCloudCode` is `&'static dyn`.

mod envelope;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::google_genai::decode::{decode_frame, decode_full_with};
use crate::protocol::{
    Collection, DecodeState, Frame, Framing, Method, ModelKeys, ModelsShape, Protocol, ProviderCtx,
    Shapes, Tuning, WireRequest,
};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct GoogleCloudCode;

impl Protocol for GoogleCloudCode {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError> {
        envelope::encode(req, ctx)
    }

    fn path(&self, _ctx: &ProviderCtx) -> String {
        // `--raw` has no parsed `stream`; target the streaming endpoint (brazen's
        // native mode), the same path `encode` builds for a streaming request.
        envelope::request_path(true).to_owned()
    }

    fn content_type(&self) -> &str {
        "application/json"
    }

    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
        decode_frame(frame, state, envelope::unwrap)
    }

    fn decode_full(
        &self,
        body: &[u8],
        state: &mut DecodeState,
    ) -> Result<Vec<Event>, CanonicalError> {
        decode_full_with(body, state, envelope::unwrap)
    }

    fn framing(&self) -> Framing {
        Framing::Sse
    }

    /// §4's values: the inner body IS `google_genai`'s, so every claim it makes holds
    /// here byte-for-byte (`protocol_tuning`/`protocol_shapes` prove it against `encode`).
    fn tuning(&self) -> Tuning {
        Tuning {
            effort: true,
            priority: false,
            image: true,
        }
    }

    fn shapes(&self) -> Shapes {
        Shapes {
            tools: true,
            multi_turn: true,
        }
    }

    /// The backend lists models by a bodiless `POST :fetchAvailableModels` (its GET is a
    /// 404) returning a MAP keyed by id (`Collection::Map`) that names its default at the top
    /// level (`defaultAgentModelId`) and serves the limits + label per entry — all DATA
    /// for the one generic decoder (providers §4.10, model-discovery §3.1). `count_tokens`
    /// is the trait's default decline: untested here, so unclaimed.
    fn models_shape(&self) -> Option<ModelsShape> {
        Some(ModelsShape {
            method: Method::Post,
            path: "/v1internal:fetchAvailableModels",
            keys: ModelKeys {
                collection: Collection::Map { key: "models" },
                strip: "",
                context_key: "maxTokens",
                max_output_key: "maxOutputTokens",
                display_name_key: "displayName",
                default_key: "defaultAgentModelId",
            },
        })
    }
}
