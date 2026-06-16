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
/// connect/DNS/TLS/timeout failure is a `Transport` error → 69. The connect and
/// response-header bounds are NOT baked here — they ride on `wire.timeouts` from
/// the resolved config (config §4) and are applied per request below, so the bin
/// holds no magic timeout constants.
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();
        HttpTransport {
            agent: config.into(),
        }
    }
}

impl Transport for HttpTransport {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        let t = wire.timeouts;
        // Per-request overrides over the agent config (which keeps
        // `http_status_as_error(false)`); an unset bound is simply not configured.
        let mut cfg = self.agent.post(&wire.url).config();
        if let Some(secs) = t.connect {
            cfg = cfg.timeout_connect(Some(Duration::from_secs(secs)));
        }
        if let Some(secs) = t.response {
            cfg = cfg.timeout_recv_response(Some(Duration::from_secs(secs)));
        }
        let mut req = cfg.build();
        for (name, value) in &wire.headers {
            req = req.header(name, value);
        }
        let resp = req
            .send(&wire.body[..])
            .map_err(|e| transport_error(&e.to_string()))?;
        let status = resp.status().as_u16();
        let reader = resp.into_body().into_reader();
        // The streaming body is otherwise unbounded; `idle` (inter-chunk) bounds a
        // mid-stream stall without capping total length. ureq's `timeout_recv_body`
        // is a TOTAL cap (wrong for a long generation), so the bound is enforced
        // here, off-thread, resetting on every chunk (`IdleChunkReader`).
        let body: Box<dyn Iterator<Item = io::Result<Bytes>>> = match t.idle {
            Some(secs) => Box::new(IdleChunkReader::spawn(reader, Duration::from_secs(secs))),
            None => Box::new(ChunkReader { reader }),
        };
        Ok(TransportResponse { status, body })
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

/// A `ChunkReader` with an INTER-CHUNK idle bound. A blocking `read` can't be
/// interrupted in place, so a worker thread owns the `Read` and pushes each chunk
/// down a rendezvous channel; `next` waits at most `idle` for the next one. The
/// timer restarts on every received chunk, so total stream length is unbounded —
/// only a *stall* trips it, surfacing as a `TimedOut` item (`run` → Transport, 69).
/// On a stall the worker is abandoned (it dies with the one-shot process, arch
/// §10); a `None`/error from the channel means the worker reached EOF or dropped.
struct IdleChunkReader {
    rx: std::sync::mpsc::Receiver<io::Result<Bytes>>,
    idle: Duration,
    done: bool,
}

impl IdleChunkReader {
    fn spawn<R: Read + Send + 'static>(reader: R, idle: Duration) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<io::Result<Bytes>>(0);
        std::thread::spawn(move || {
            let mut reader = reader;
            loop {
                let mut buf = vec![0u8; 8192];
                let item = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        Ok(buf)
                    }
                    Err(e) => Err(e),
                };
                let is_err = item.is_err();
                // A send error means `next` stopped pulling (stall abandon / drop);
                // the worker then exits. An error chunk ends the stream too.
                if tx.send(item).is_err() || is_err {
                    break;
                }
            }
        });
        IdleChunkReader {
            rx,
            idle,
            done: false,
        }
    }
}

impl Iterator for IdleChunkReader {
    type Item = io::Result<Bytes>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.rx.recv_timeout(self.idle) {
            Ok(item) => {
                self.done = item.is_err();
                Some(item)
            }
            // No chunk within `idle`: the stream stalled mid-body.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.done = true;
                Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "stream stalled: no data within the idle-read timeout",
                )))
            }
            // The worker reached EOF (or died): a clean end of body.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.done = true;
                None
            }
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
