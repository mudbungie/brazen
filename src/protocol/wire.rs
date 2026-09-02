//! The wire REQUEST that flows encode → auth → transport (arch §4.1), and the three
//! data facts it carries about how to deliver it: the HTTP [`Method`], the optional
//! subprocess [`ExecSpec`], and the [`Envelope`] saying what that subprocess's pipes
//! carry. Kept apart from the `Protocol` seam in the parent — the trait is the
//! dialect's BEHAVIOUR, this is the request it hands over.

use crate::transport::Timeouts;

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
