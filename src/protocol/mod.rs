//! The protocol seam (arch §4.1): the `Protocol` trait owning a wire dialect, the
//! secret-free `ProviderCtx` handed to encode/auth, and the `WireRequest` that
//! flows encode → auth → transport. The framing types live in `frame`; concrete
//! protocol impls (openai_chat, anthropic_messages) and the framers plug in via
//! their own tasks.

pub mod frame;

use serde_json::{Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::config::provider::HeaderSpec;

pub use frame::{DecodeState, Frame, Framing, OpenBlock};

/// The HTTP request that flows encode → auth → transport (arch §4.1). `encode`
/// builds the body + non-auth headers; `Auth::apply` adds the auth headers in
/// place; `Transport::send` consumes it. Header names match case-insensitively so
/// an auth overwrite never duplicates a header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl WireRequest {
    /// A request targeting `url` with `body`, no headers yet.
    pub fn new(url: impl Into<String>, body: Vec<u8>) -> Self {
        WireRequest {
            url: url.into(),
            headers: Vec::new(),
            body,
        }
    }

    /// The `--raw` constructor: stdin bytes verbatim, no url/headers yet (arch §4.4).
    pub fn raw(body: Vec<u8>) -> Self {
        WireRequest::new(String::new(), body)
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
/// `encode` and `auth` (arch §4.1) — the ENTIRE interface between "which provider"
/// and "how to talk to it". No name, no `ProtocolId`/`AuthId`, no secret: the
/// vendor identity was spent on the registry lookup before these run.
pub struct ProviderCtx<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_header: &'a HeaderSpec,
    pub beta_headers: &'a [(&'a str, &'a str)],
    pub extra: &'a Map<String, Value>,
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

    /// Consume ONE already-parsed frame → zero or more canonical events. All
    /// cross-frame state is the caller-owned `DecodeState`, so the impl is a pure
    /// fn of `(frame, state)` and shareable as `&'static dyn`.
    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError>;

    /// Which transport framing this protocol uses — DATA, not behaviour.
    fn framing(&self) -> Framing;
}
