//! The protocol seam (arch §4.1): the `Protocol` trait owning a wire dialect, the
//! secret-free `ProviderCtx` handed to encode/auth, and the `WireRequest` that
//! flows encode → auth → transport (in `wire`). The framing types live in `frame`; the five
//! concrete protocol impls are `anthropic` (Messages), `openai` (Chat Completions),
//! `openai_responses` (Responses), `google_genai`, and `ollama_chat`. The framers
//! live in `sse`.

pub mod anthropic;
pub mod claude_code;
pub mod frame;
pub mod google_genai;
mod json;
pub mod ollama_chat;
pub mod openai;
pub mod openai_responses;
pub mod sse;
mod synth;
mod wire;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};

pub use frame::{DecodeState, Decoder, Frame, Framing, OpenBlock};
/// The ONE whole-body non-2xx HTTP error projection + the ONE generic models-list
/// decoder + the ONE generic token-count decoder (json.rs). `http_error` drains a
/// provider error body and carries it VERBATIM; `decode_models` projects a models-list
/// body onto `Vec<Model>` reading the `(array_key, id_key, strip)` a protocol's
/// [`ModelsShape`] supplies (overridden per row, model-discovery §3.2); `count_from_body`
/// reads the token count from a 2xx count body at the response key a [`CountRequest`]
/// supplies. The data plane's error fold reaches `http_error` through `decode`; the
/// model-discovery path (`run::models`) and the count path (`run::count`) route their
/// non-2xx round-trips through the SAME home and call the decoders directly (`json` is
/// private).
pub(crate) use json::{count_from_body, decode_models, http_error};
/// The wire request + the three delivery facts it carries (`wire.rs`): the HTTP
/// `Method`, the subprocess `ExecSpec`, and the `Envelope` its pipes carry.
pub use wire::{Envelope, ExecSpec, Method, WireRequest};

/// The per-list-body projection keys the generic `decode_models` reads (model-discovery
/// §3): the top-level `array_key` array, and per entry the wire `id_key` (with the leading
/// `strip` removed) plus the OPTIONAL metadata key paths — `context_key` (input token
/// limit → `Model.context_window`), `max_output_key` (output limit → `max_output_tokens`),
/// `display_name_key` (→ `display_name`). Each metadata key is `""` when the dialect (or a
/// row override) does not serve that fact, so the `Model` field stays `None`, NEVER
/// fabricated (the Usage zero-vs-unknown principle, AGENTS.md). This struct is the SINGLE
/// home for the decode key set: it is the defaults embedded in [`ModelsShape`] AND the
/// resolved keys `models_req` hands `decode_models`, so it borrows either the `&'static`
/// protocol shape or a row's `'a` `[provider.models]` override (§3.2) — no second list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelKeys<'a> {
    pub array_key: &'a str,
    pub id_key: &'a str,
    pub strip: &'a str,
    pub context_key: &'a str,
    pub max_output_key: &'a str,
    pub display_name_key: &'a str,
}

/// A dialect's models-list shape as DATA (model-discovery §3.1): the GET `path` appended
/// to `base_url`, plus the default projection `keys`. `path` and the overridable members
/// of `keys` (`array_key`/`id_key` and the metadata keys) are the protocol DEFAULTS a
/// row's `[provider.models]` block may override (§3.2); `strip` is protocol-only. `&'static
/// str` throughout — every value is a compile-time constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelsShape {
    pub path: &'static str,
    pub keys: ModelKeys<'static>,
}

/// Which canonical TUNING knobs a dialect PROJECTS onto its wire, as DATA (config
/// §6.1) — the [`ModelsShape`] pattern applied to request shaping. The fact lives
/// beside the `encode` that implements it, so the READ surface
/// (`bz --list-providers`) never re-derives it from a match on the protocol id, and a
/// consumer above brazen never keeps its own copy of one. There is deliberately NO
/// default impl: a new dialect must answer for itself, where a default would silently
/// claim (or deny) a capability its `encode` has not implemented. The claim is not
/// taken on trust — `src/tests/protocol_tuning.rs` proves each flag against the dialect's
/// OWN `encode`, key-agnostically: setting the knob changes the encoded request iff
/// the dialect projects it.
/// Derives are exactly what is used: `Debug`/`PartialEq` for the cross-check test.
/// It is not `Serialize` — the LISTING serializes its own `Row` booleans, and a second
/// serializable shape of one fact is the drift this crate refuses.
#[derive(Debug, PartialEq)]
pub struct Tuning {
    /// The dialect has a wire shape for `req.reasoning` (providers.md §6) — every
    /// shipped dialect does, under five irreconcilable spellings.
    pub effort: bool,
    /// The dialect has a `service_tier` wire spelling for `req.service_tier`
    /// (providers.md §6.2) — the OpenAI family and Anthropic; Google/Ollama/
    /// claude_code narrow it away.
    pub priority: bool,
}

/// A dialect's token-count round-trip (architecture §5.10.1, bl-24e5): the POST
/// [`WireRequest`] targeting the count endpoint (URL + body built from the SAME
/// message/system/tool projection the dialect's `encode` uses) plus the response's
/// token-count JSON key (`input_tokens` Anthropic, `totalTokens` Google). Returned by
/// [`Protocol::count_tokens`]; the count runner stamps `content_type`/betas/auth (as
/// `serve` does), sends once, and reads `token_key` from the 2xx body via
/// [`count_from_body`]. Not the pure-data twin of [`ModelsShape`] — the count body is a
/// per-dialect projection of the request, not a static path — so the seam carries the
/// built request, not just keys.
pub struct CountRequest {
    pub wire: WireRequest,
    pub token_key: &'static str,
}

/// The read-only, secret-free projection of the resolved row + flags handed to
/// `encode` (arch §4.1) — the ENTIRE interface between "which provider" and "how to
/// talk to it". No name, no `ProtocolId`/`AuthId`, no secret, and no `api_header`:
/// the auth header is auth's concern (it rides `AuthCtx`), and the vendor identity
/// was spent on the registry lookup before these run. The body-passthrough valve is
/// NOT here: config-level passthrough (top-level `extra` + a row's non-gen
/// `body_defaults`) is folded into `req.extra` by `fill_absent` and reaches the wire
/// through the one `req.extra` fold every encoder already runs (config §4.1, §9).
pub struct ProviderCtx<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub beta_headers: &'a [(&'a str, &'a str)],
    /// The row's subprocess program (claude-code spec §7.1), `Some` exactly when the
    /// row carries `exec`. Read only by an exec-transport dialect's `encode`/
    /// [`Protocol::exec_spec`]; the HTTP dialects never consult it (the empty-set rule).
    pub exec: Option<&'a str>,
}

/// A wire dialect (arch §4.1): pure — no IO, no clock, no creds. `encode` projects
/// the canonical request onto the wire; `decode` is a pure `(frame, state)` state
/// machine yielding canonical events; `framing` declares the transport framing as
/// data. Object-safe: the pipeline holds `&dyn Protocol`.
pub trait Protocol: Send + Sync {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError>;

    /// The request path appended to `base_url` to form the target URL (e.g.
    /// `/responses`, `/api/chat`). `encode` builds its own `wire.url` from this
    /// SAME path (single source — the path string has one home); the `--raw` spine
    /// (arch §4.4), which skips `encode` and so has no parsed body to encode, calls
    /// this to fill `wire.url`. Google's path carries the model segment and a stream
    /// verb — `--raw` has no parsed `stream`, so it targets the streaming endpoint
    /// (brazen's native mode).
    fn path(&self, ctx: &ProviderCtx) -> String;

    /// The `Content-Type` the wire body carries — DATA, like `path`/`models_path`.
    /// A dialect fact with ONE home: `serve` stamps it onto the `WireRequest` for
    /// BOTH the encoded and the `--raw` paths (arch §4.4), so neither `encode` nor
    /// the raw arm hardcodes the string. Every shipped protocol is JSON today
    /// (`application/json`); a future non-JSON dialect overrides just this one method.
    fn content_type(&self) -> &str;

    /// Consume ONE already-parsed frame → zero or more canonical events. All
    /// cross-frame state is the caller-owned `DecodeState`, so the impl is a pure
    /// fn of `(frame, state)` and shareable as `&'static dyn`.
    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError>;

    /// Decode a COMPLETE non-stream 2xx body → the SAME canonical events the
    /// streamed form yields (message_start .. finish; never `End` — run owns it,
    /// like `decode`). Honoring `stream:false` (config §4.2) is NOT a second parser:
    /// a non-stream body is the AGGREGATE the stream emits, so each impl reconstructs
    /// the synthetic event sequence the stream would have produced and REPLAYS it
    /// through the protocol's own `decode`-internal helpers (`event`/`chunk`/`line` +
    /// `terminal`/`synth`). e.g. an `openai_responses` body IS the `response` object
    /// streaming's `response.completed` wraps, so it reuses `terminal::{completed,…}`
    /// verbatim; the structureless dialects replay one synthetic terminal chunk. Pure,
    /// fixture-tested like `decode` — `run`'s `whole_body_success` fold calls it on a
    /// `!streamed` 2xx body (no premature-EOF check: the body is complete).
    fn decode_full(
        &self,
        body: &[u8],
        state: &mut DecodeState,
    ) -> Result<Vec<Event>, CanonicalError>;

    /// Which transport framing this protocol uses — DATA, not behaviour.
    fn framing(&self) -> Framing;

    /// Which canonical tuning knobs this dialect projects — DATA, like `framing`
    /// (config §6.1). Read by `bz --list-providers`, which pairs it with the row's
    /// `unsupported_body_keys` to answer "can THIS row take `--reasoning`/`--tier`?".
    fn tuning(&self) -> Tuning;

    /// The dialect's models-discovery DEFAULTS as DATA, like `path` (model-discovery
    /// §3.1): the GET `path` appended to `base_url`, the top-level `array_key`, the
    /// per-entry `id_key`, and Google's leading-`models/` `strip`. There is no
    /// per-protocol `decode_models` method — the `list-models` verb feeds these
    /// defaults (OVERRIDDEN per row by `[provider.models]`, §3.2) to the ONE generic
    /// [`decode_models`], which projects the body onto an ORDER-PRESERVING `Vec<Model>`.
    /// `None` = this dialect HAS no models listing (the `count_tokens` decline shape,
    /// claude-code spec §7.2): the verb fails with a `Config` error naming the next
    /// move; a row's `[provider.models]` override cannot conjure a listing over it.
    fn models_shape(&self) -> Option<ModelsShape>;

    /// The dialect's subprocess target as DATA (claude-code spec §3.1) — the exec
    /// sibling of [`Protocol::path`]. `Some` = this dialect rides the exec transport;
    /// the `--raw` spine (which skips `encode`) stamps `wire.exec` from it exactly as
    /// it fills `wire.url` from `path`. The **default is `None`** — every HTTP dialect
    /// needs zero code.
    fn exec_spec(&self, ctx: &ProviderCtx) -> Option<ExecSpec> {
        let _ = ctx;
        None
    }

    /// Project the canonical request onto this dialect's token-count endpoint
    /// (architecture §5.10.1, bl-24e5) — the `--count-tokens` control op. `None` = this
    /// dialect has NO count endpoint, so the op DECLINES with a `Config` error (a
    /// fabricated estimate is a lie; §8). `Some(Ok(..))` carries the built
    /// [`CountRequest`]; `Some(Err(..))` is an encode failure (e.g. non-representable
    /// content), surfaced like any encode error. The **default is the decline** — a
    /// dialect opts in by overriding, reusing its own `encode` projection (Anthropic
    /// drops the generation-only keys; Google wraps in a `generateContentRequest`), so a
    /// dialect with no count endpoint needs zero code.
    fn count_tokens(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Option<Result<CountRequest, CanonicalError>> {
        let _ = (req, ctx);
        None
    }
}
