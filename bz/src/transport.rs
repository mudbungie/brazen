//! The native HTTP transport (arch §4.1, §9.1, §10) — the ONLY user of `ureq` in
//! the workspace. It lives in the `bz` bin crate, never the `brazen` lib, so the
//! pure, network-free core literally cannot link the network client: the invariant
//! is enforced by the crate graph, not by a comment + discipline (bl-c420). Behind
//! the lib's `Transport` seam the lib reaches 100% coverage via `MockTransport`;
//! this file is coverage-excluded with the rest of the shim (Makefile `cov`).

use std::io::{self, Read};
use std::time::Duration;

use brazen::{Bytes, CanonicalError, ErrorKind, Transport, TransportResponse, WireRequest};

/// The real network seam (arch §4.1, §9.1, §10) — a blocking, rustls-backed HTTP
/// round-trip via `ureq`. `http_status_as_error(false)` so a non-2xx flows on as a
/// normal response (the pipeline derives the exit from the status); only a
/// connect/DNS/TLS/timeout failure is a `Transport` error → 69.
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .build();
        HttpTransport {
            agent: config.into(),
        }
    }
}

impl Transport for HttpTransport {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        let mut req = self.agent.post(&wire.url);
        for (name, value) in &wire.headers {
            req = req.header(name, value);
        }
        let resp = req
            .send(&wire.body[..])
            .map_err(|e| transport_error(&e.to_string()))?;
        let status = resp.status().as_u16();
        let reader = resp.into_body().into_reader();
        Ok(TransportResponse {
            status,
            body: Box::new(ChunkReader { reader }),
        })
    }
}

/// Adapts ureq's blocking body `Read` into the seam's incremental body stream
/// (`Iterator<Item = io::Result<Bytes>>`): each `next` is one `read` into a fresh
/// buffer — `Ok(0)` is EOF (`None`), a short read yields just what arrived (never
/// buffered to end, so the pipeline streams chunk-by-chunk), and a read error
/// surfaces as the item (`run` maps a mid-stream drop to a `Transport` exit 69).
struct ChunkReader<R> {
    reader: R,
}

impl<R: Read> Iterator for ChunkReader<R> {
    type Item = io::Result<Bytes>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = vec![0u8; 8192];
        match self.reader.read(&mut buf) {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some(Ok(buf))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

/// A connect/TLS/timeout failure as a `Transport`-kind `CanonicalError` (arch §8
/// → exit 69). No `provider_detail`: there is no upstream response to carry.
fn transport_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("HTTP transport: {message}"),
        provider_detail: None,
    }
}
