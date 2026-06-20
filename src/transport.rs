//! The transport seam (arch §4.1, §9.1): the ONE impure surface. `bz` wires the
//! rustls-backed `HttpTransport`; tests wire `MockTransport`. A blocking,
//! incremental body iterator keeps the pipeline a pure `Iterator`, never async.

use std::io;

use crate::canonical::CanonicalError;
use crate::protocol::WireRequest;

/// One transport body chunk. An alias, so the framers' `Vec<u8>` and the body
/// stream speak the same type.
pub type Bytes = Vec<u8>;

/// The peeked HTTP status (read even under `--raw`, for exit-code correctness)
/// plus the blocking, incremental body stream (arch §4.1).
pub struct TransportResponse {
    pub status: u16,
    pub body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
}

/// The per-request transport timeouts (config §4), in WHOLE SECONDS; each `None`
/// leaves that bound unset (the transport's own default). Carried on the
/// [`WireRequest`] — the one thing crossing the
/// seam — so config-sourced policy reaches the impure transport without widening
/// the `send` signature. `bz` derives all three from the resolved config (whose
/// floor is `data/defaults.toml`), so the numbers live in config, never as magic
/// in the bin (severability — policy in config, not core).
///
/// - `connect`: cap on establishing the connection (DNS/TCP/TLS).
/// - `response`: cap on awaiting the response headers — **not** the body.
/// - `idle`: the INTER-CHUNK bound on the streaming body, reset on every chunk.
///   It bounds a provider that sends headers then stalls mid-stream without
///   capping total stream length, so a long-but-live generation is never
///   truncated (a *total* body cap, ureq's `timeout_recv_body`, would be wrong).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Option<u64>,
    pub response: Option<u64>,
    pub idle: Option<u64>,
}

/// The single network seam (arch §4.1). Object-safe; `Send + Sync` so an impl is
/// shareable. Exactly one round-trip per process — a caller wanting N concurrent
/// requests spawns N `bz`.
pub trait Transport: Send + Sync {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError>;
}
