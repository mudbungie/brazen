//! Shared test doubles for the injected seams (arch §9.1): `MockTransport` feeds a
//! canned response (with an optional injected mid-stream error) and captures the
//! requests it was sent; `MemoryCredStore` is an in-process `CredStore`;
//! `FakeClock` drives fresh/stale branches with no real time. Always compiled (the
//! crate is internal, `publish = false`) and free of `unwrap`/`expect`/`panic` so
//! it passes the data-path lint even under `not(test)`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::auth::login::{BrowserLauncher, CodeReceiver, Pacer};
use crate::canonical::CanonicalError;
use crate::protocol::WireRequest;
use crate::store::{Clock, Cred, CredStore};
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
    seen: Mutex<Vec<WireRequest>>,
}

impl MockTransport {
    /// A transport that answers with `status` and replays `chunks` as the body.
    pub fn new(status: u16, chunks: Vec<Chunk>) -> Self {
        MockTransport {
            status,
            chunks,
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
        })
    }
}

/// An in-process `CredStore` (arch §9.1) backing the data-plane auth tests.
#[derive(Default)]
pub struct MemoryCredStore {
    creds: RefCell<HashMap<String, Cred>>,
}

impl MemoryCredStore {
    /// An empty store.
    pub fn new() -> Self {
        MemoryCredStore::default()
    }

    /// A store preloaded with one provider's cred.
    pub fn with(provider: &str, cred: Cred) -> Self {
        let store = MemoryCredStore::new();
        store.creds.borrow_mut().insert(provider.to_owned(), cred);
        store
    }
}

impl CredStore for MemoryCredStore {
    fn get(&self, provider: &str) -> Option<Cred> {
        self.creds.borrow().get(provider).cloned()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        self.creds
            .borrow_mut()
            .insert(provider.to_owned(), cred.clone());
        Ok(())
    }
}

/// A `Clock` whose time is set explicitly — drives fresh/stale branches and
/// device-flow deadlines with no real time (arch §9.4).
pub struct FakeClock {
    now: Cell<u64>,
}

impl FakeClock {
    /// A clock reading `now` unix-seconds.
    pub fn new(now: u64) -> Self {
        FakeClock {
            now: Cell::new(now),
        }
    }

    /// Jump the clock to `now`.
    pub fn set(&self, now: u64) {
        self.now.set(now);
    }

    /// Advance the clock by `secs` seconds.
    pub fn advance(&self, secs: u64) {
        self.now.set(self.now.get() + secs);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> u64 {
        self.now.get()
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
        })
    }
}

/// A `BrowserLauncher` that RECORDS the url it was asked to open and never execs
/// (auth §7.2, §8) — the argv/url is asserted as data.
#[derive(Default)]
pub struct FakeBrowserLauncher {
    opened: Mutex<Vec<String>>,
}

impl FakeBrowserLauncher {
    /// A launcher that records every opened url.
    pub fn new() -> Self {
        FakeBrowserLauncher::default()
    }

    /// The urls `open` was called with, in order.
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .ok()
            .map(|o| o.to_vec())
            .unwrap_or_default()
    }
}

impl BrowserLauncher for FakeBrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()> {
        if let Ok(mut opened) = self.opened.lock() {
            opened.push(url.to_owned());
        }
        Ok(())
    }
}

/// A `CodeReceiver` that reports a fixed `port` and returns a canned callback query
/// (auth §7.2, §8) — no socket, no thread.
pub struct FakeCodeReceiver {
    port: u16,
    query: String,
}

impl FakeCodeReceiver {
    /// A receiver bound (notionally) to `port`, returning `query` from `await_query`.
    pub fn new(port: u16, query: impl Into<String>) -> Self {
        FakeCodeReceiver {
            port,
            query: query.into(),
        }
    }
}

impl CodeReceiver for FakeCodeReceiver {
    fn port(&self) -> u16 {
        self.port
    }

    fn await_query(&self) -> io::Result<String> {
        Ok(self.query.clone())
    }
}

/// A `Pacer` that records the intervals it was asked to wait and returns instantly
/// (auth §7.3, §8) — proving `slow_down` raises the interval with no real sleep.
#[derive(Default)]
pub struct FakePacer {
    waited: Mutex<Vec<u64>>,
}

impl FakePacer {
    /// A pacer that sleeps for nothing but records each requested interval.
    pub fn new() -> Self {
        FakePacer::default()
    }

    /// The intervals `wait` was called with, in order.
    pub fn waited(&self) -> Vec<u64> {
        self.waited
            .lock()
            .ok()
            .map(|w| w.to_vec())
            .unwrap_or_default()
    }
}

impl Pacer for FakePacer {
    fn wait(&self, secs: u64) {
        if let Ok(mut waited) = self.waited.lock() {
            waited.push(secs);
        }
    }
}
