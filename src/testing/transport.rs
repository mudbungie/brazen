//! `Transport` doubles (arch §9.1): `MockTransport` replays one canned response
//! (with an optional injected mid-stream error) and captures the requests it was
//! sent; `ScriptedTransport` answers each `send` with the next response in a queue,
//! so the device-flow poll loop drives through `pending → slow_down → success`.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::canonical::CanonicalError;
use crate::protocol::WireRequest;
use crate::transport::{Transport, TransportResponse};

/// One canned body chunk: bytes, or an injected mid-stream IO failure (a transport
/// drop is just an `Err` element — arch §9.1).
#[derive(Clone)]
pub enum Chunk {
    Data(Vec<u8>),
    Fail(io::ErrorKind),
}

/// A `Transport` returning a fixed status and a canned body, capturing every
/// `WireRequest` it is sent so a test can assert encode+auth end to end.
pub struct MockTransport {
    status: u16,
    chunks: Vec<Chunk>,
    retry_after: Option<String>,
    seen: Mutex<Vec<WireRequest>>,
}

impl MockTransport {
    /// A transport that answers with `status` and replays `chunks` as the body.
    pub fn new(status: u16, chunks: Vec<Chunk>) -> Self {
        MockTransport {
            status,
            chunks,
            retry_after: None,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// A 200 response whose body is the given byte chunks, no injected error.
    pub fn ok(chunks: Vec<&[u8]>) -> Self {
        let chunks = chunks
            .into_iter()
            .map(|c| Chunk::Data(c.to_vec()))
            .collect();
        MockTransport::new(200, chunks)
    }

    /// Answer with a `Retry-After` response header (arch §3.3) — the transport-fact
    /// knob mirroring the real seam's header capture. The value is the header VERBATIM
    /// (integer seconds or an HTTP-date); the lib parses it against the `Clock`.
    pub fn with_retry_after(mut self, header: impl Into<String>) -> Self {
        self.retry_after = Some(header.into());
        self
    }

    /// Every `WireRequest` this transport was sent, in order.
    pub fn requests(&self) -> Vec<WireRequest> {
        self.seen
            .lock()
            .ok()
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }
}

impl Transport for MockTransport {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(wire);
        }
        let body = self.chunks.clone().into_iter().map(|c| match c {
            Chunk::Data(bytes) => Ok(bytes),
            Chunk::Fail(kind) => Err(io::Error::from(kind)),
        });
        Ok(TransportResponse {
            status: self.status,
            body: Box::new(body),
            retry_after: self.retry_after.clone(),
        })
    }
}

/// A `Transport` that answers each `send` with the NEXT canned response from a
/// queue (status + body chunks) — so the device-flow poll loop can be driven
/// through `authorization_pending → slow_down → success` (auth §8). Once the queue
/// is exhausted it repeats the last response. Captures every request like
/// `MockTransport`.
pub struct ScriptedTransport {
    responses: Vec<(u16, Vec<u8>)>,
    next: AtomicUsize,
    seen: Mutex<Vec<WireRequest>>,
}

impl ScriptedTransport {
    /// A transport replaying `responses` (each a `(status, body)`), one per `send`.
    pub fn new(responses: Vec<(u16, Vec<u8>)>) -> Self {
        ScriptedTransport {
            responses,
            next: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Every `WireRequest` this transport was sent, in order.
    pub fn requests(&self) -> Vec<WireRequest> {
        self.seen
            .lock()
            .ok()
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }
}

impl Transport for ScriptedTransport {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(wire);
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let idx = n.min(self.responses.len().saturating_sub(1));
        let (status, body) = self.responses[idx].clone();
        Ok(TransportResponse {
            status,
            body: Box::new(std::iter::once(Ok(body))),
            retry_after: None,
        })
    }
}
