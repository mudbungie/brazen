//! The `bz --login` control-plane doubles (auth §7.2, §7.3, §8): a `BrowserLauncher`
//! that records the url and never execs, a `CodeReceiver` that returns a canned
//! callback query with no socket, and a `Pacer` that records intervals and sleeps
//! for nothing — so the whole interactive flow runs offline with no real time.

use std::io;
use std::sync::Mutex;

use crate::auth::login::{BrowserLauncher, CodeReceiver, Pacer};

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

/// A `CodeReceiver` that binds (notionally) and returns a canned callback query
/// (auth §7.2, §8, §10.1) — no socket, no thread. `bind` honors a requested fixed
/// port (echoing it back, proving the row's `redirect.port` flowed) and falls back
/// to the fixed `port` for the ephemeral (`None`) case.
pub struct FakeCodeReceiver {
    port: u16,
    query: String,
}

impl FakeCodeReceiver {
    /// A receiver whose ephemeral bind reports `port`, returning `query` from
    /// `await_query`.
    pub fn new(port: u16, query: impl Into<String>) -> Self {
        FakeCodeReceiver {
            port,
            query: query.into(),
        }
    }
}

impl CodeReceiver for FakeCodeReceiver {
    fn bind(&self, port: Option<u16>) -> io::Result<u16> {
        Ok(port.unwrap_or(self.port))
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
