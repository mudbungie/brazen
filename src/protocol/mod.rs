//! The protocol seam (arch §4.1): the `Protocol` trait owning a wire dialect, the
//! secret-free `ProviderCtx` handed to encode/auth, and the `WireRequest` that
//! flows encode → auth → transport. The framing types live in `frame`; the five
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

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::transport::Timeouts;

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

/// The HTTP verb a `WireRequest` carries (model-discovery §6): every generation
/// request is a `Post` (the default — `encode` is unchanged), the `list-models` verb's
/// GET a `Get`. Data on the one struct already crossing the transport seam (mirrors
/// `timeouts`), not a new `send` parameter — the impure `HttpTransport` reads it to
/// pick the verb, `MockTransport` records it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Method {
    #[default]
    Post,
    Get,
}

/// A subprocess target a [`WireRequest`] may name instead of an HTTP one
/// (claude-code spec §3.1): the native transport spawns `program args…`, writes
/// `wire.body` to the child's stdin, and streams the child's stdout as the response
/// body. Data on the one struct already crossing the transport seam — like
/// [`Method`]/[`Timeouts`], never a new `send` parameter. [`Envelope`] says what the
/// child's pipes CARRY, which is the only thing the two subprocess uses differ in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecSpec {
    pub program: String,
    pub args: Vec<String>,
    pub envelope: Envelope,
}

/// What a spawned child's stdin/stdout carry (transport spec §4.1) — the ONE
/// discriminator between the two subprocess uses, so `WireRequest` never grows a
/// second exec field and a row can never be both by construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Envelope {
    /// The child IS the provider: stdin is the dialect's own body, stdout its own
    /// dialect stream, status 200 at spawn (claude-code spec §3.2).
    #[default]
    Body,
    /// The child IS the transport: stdin is one whole HTTP/1.1 request message,
    /// stdout one whole HTTP/1.1 response message (transport spec §5). The status,
    /// and any `retry-after`, are the ones the child reports.
    Http,
}

/// The HTTP request that flows encode → auth → transport (arch §4.1). `encode`
/// builds the body + non-auth headers; `Auth::apply` adds the auth headers in
/// place; `Transport::send` consumes it. Header names match case-insensitively so
/// an auth overwrite never duplicates a header. `method` is `Post` for every
/// generation request (the default — `encode` builds POSTs via `new`) and `Get` for
/// the `list-models` verb's GET (§6). `timeouts` is the per-request transport policy
/// (config §4): `encode` leaves it at the `Default` (all unset) and `run` stamps the
/// resolved config onto it before `send`, so a config-driven bound reaches the
/// impure transport without a wider `send` signature. `exec` declares a SUBPROCESS
/// target (claude-code spec §3): `None` = HTTP (every prior dialect, byte-identical);
/// `Some` routes the native transport to the spawn — `url`/`method`/`headers` are
/// inert on that path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeouts: Timeouts,
    pub exec: Option<ExecSpec>,
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
            exec: None,
        }
    }

    /// A `Get` request targeting `url` with an empty body — the `list-models` verb's
    /// GET (§6). No headers yet and default (unset) timeouts.
    pub fn get(url: impl Into<String>) -> Self {
        WireRequest {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
            timeouts: Timeouts::default(),
            exec: None,
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
