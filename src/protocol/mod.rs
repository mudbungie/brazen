//! The protocol seam (arch §4.1): the `Protocol` trait owning a wire dialect, the
//! secret-free `ProviderCtx` handed to encode/auth, and the `WireRequest` that
//! flows encode → auth → transport. The framing types live in `frame`; concrete
//! protocol impls (openai_chat, anthropic_messages) and the framers plug in via
//! their own tasks.

pub mod anthropic;
pub mod frame;
pub mod google_genai;
mod json;
pub mod ollama_chat;
pub mod openai;
pub mod openai_responses;
pub mod sse;
mod synth;

use crate::canonical::{CanonicalError, CanonicalRequest, Event, Model};
use crate::transport::Timeouts;

pub use frame::{DecodeState, Decoder, Frame, Framing, OpenBlock};

/// The HTTP verb a `WireRequest` carries (model-discovery §6): every generation
/// request is a `Post` (the default — `encode` is unchanged), the models-list probe
/// a `Get`. Data on the one struct already crossing the transport seam (mirrors
/// `timeouts`), not a new `send` parameter — the impure `HttpTransport` reads it to
/// pick the verb, `MockTransport` records it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Method {
    #[default]
    Post,
    Get,
}

/// The HTTP request that flows encode → auth → transport (arch §4.1). `encode`
/// builds the body + non-auth headers; `Auth::apply` adds the auth headers in
/// place; `Transport::send` consumes it. Header names match case-insensitively so
/// an auth overwrite never duplicates a header. `method` is `Post` for every
/// generation request (the default — `encode` builds POSTs via `new`) and `Get` for
/// the models-list probe (§6). `timeouts` is the per-request transport policy
/// (config §4): `encode` leaves it at the `Default` (all unset) and `run` stamps the
/// resolved config onto it before `send`, so a config-driven bound reaches the
/// impure transport without a wider `send` signature.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeouts: Timeouts,
}

impl WireRequest {
    /// A `Post` request targeting `url` with `body`, no headers yet and default
    /// (unset) timeouts. The one constructor `encode` uses — the method stays `Post`.
    pub fn new(url: impl Into<String>, body: Vec<u8>) -> Self {
        WireRequest {
            method: Method::Post,
            url: url.into(),
            headers: Vec::new(),
            body,
            timeouts: Timeouts::default(),
        }
    }

    /// A `Get` request targeting `url` with an empty body — the models-list probe
    /// and `list-models` verb (§6). No headers yet and default (unset) timeouts.
    pub fn get(url: impl Into<String>) -> Self {
        WireRequest {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
            timeouts: Timeouts::default(),
        }
    }

    /// Set a header, replacing any existing one of the same (case-insensitive)
    /// name rather than appending a duplicate.
    pub fn set_header(&mut self, name: &str, value: &str) {
        if let Some(slot) = self
            .headers
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            slot.1 = value.to_owned();
        } else {
            self.headers.push((name.to_owned(), value.to_owned()));
        }
    }

    /// The value of a header by case-insensitive name, if set.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
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

    /// Consume ONE already-parsed frame → zero or more canonical events. All
    /// cross-frame state is the caller-owned `DecodeState`, so the impl is a pure
    /// fn of `(frame, state)` and shareable as `&'static dyn`.
    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError>;

    /// Which transport framing this protocol uses — DATA, not behaviour.
    fn framing(&self) -> Framing;

    /// The models-listing endpoint appended to `base_url` for a GET — DATA, like
    /// `path` (model-discovery §3.1). e.g. openai_chat `/models` (base ends `/v1`),
    /// anthropic `/v1/models` (bare base), google `/v1beta/models`, ollama
    /// `/api/tags`. The `list-models` verb and the imprecise-model probe target
    /// `{base_url}{models_path()}`.
    fn models_path(&self) -> &str;

    /// Decode the provider's (non-streaming) models-list body into the canonical
    /// ORDER-PRESERVING list (model-discovery §3.1). PURE — no IO, fixture-tested like
    /// `decode`. Vendor-blind: it projects the dialect's list shape onto `Vec<Model>`,
    /// preserving the provider's order (the authoritative sequence the default/partial
    /// heuristics read, §4). A malformed/unexpected body is a `Provider` error.
    fn decode_models(&self, body: &[u8]) -> Result<Vec<Model>, CanonicalError>;
}
