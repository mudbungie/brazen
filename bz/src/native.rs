//! The native impure impls behind brazen's seams (arch §6.5, §9.5, §10) — part of
//! the coverage-excluded `bz` shim (Makefile `cov` `--ignore-filename-regex 'bz/'`).
//! The native impurities live here: the system clock, the browser spawn, the
//! loopback `bind`/`accept`, and the device-poll sleep. The atomic 0600 credential
//! store is in [`creds`] and the OS RNG in [`rng`]; the rustls-backed
//! `HttpTransport` — the lone `ureq` user — is its sibling [`crate::transport`]. The
//! library reaches 100% behind injection; the pure parsing these call
//! (`browser_argv`, `query_from_request_line`, the OAuth builders) is in the lib.

mod creds;
mod rng;

pub use creds::XdgCredStore;
pub use rng::random_token;

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brazen::{BrowserLauncher, Clock, CodeReceiver, Pacer};

/// The system clock (arch §6.5): the one place real time is read. A pre-1970 clock
/// is clamped to 0 rather than panicking.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Open the authorize URL in the user's browser (auth §7.2): spawn `browser_argv`
/// (the OS→argv map is pure lib data) — the one excluded `spawn` line.
pub struct SystemBrowserLauncher;

impl BrowserLauncher for SystemBrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()> {
        let mut argv = brazen::browser_argv(url).into_iter();
        let prog = argv.next().unwrap_or_default();
        Command::new(prog).args(argv).spawn()?;
        Ok(())
    }
}

/// The RFC 8252 loopback receiver (auth §7.2, §7.4): bind `127.0.0.1:0`, accept the
/// provider's redirect, read the request line, and defer the query extraction to
/// the pure `query_from_request_line`. Only the `bind` is coverage-excluded.
pub struct LoopbackReceiver {
    listener: TcpListener,
}

impl LoopbackReceiver {
    pub fn bind() -> io::Result<Self> {
        Ok(LoopbackReceiver {
            listener: TcpListener::bind("127.0.0.1:0")?,
        })
    }
}

impl CodeReceiver for LoopbackReceiver {
    fn port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    fn await_query(&self) -> io::Result<String> {
        let (stream, _) = self.listener.accept()?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let query = brazen::query_from_request_line(line.trim_end())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "callback had no query"))?;
        let body = "brazen: you may close this tab and return to the terminal.";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut stream = stream;
        stream.write_all(resp.as_bytes())?;
        Ok(query)
    }
}

/// The device-flow poll pacer (auth §7.3): the real binary sleeps `secs`.
pub struct RealPacer;

impl Pacer for RealPacer {
    fn wait(&self, secs: u64) {
        std::thread::sleep(Duration::from_secs(secs));
    }
}

/// A `CodeReceiver` for the device flow, where no loopback is bound — its methods
/// are never reached (the device flow uses no receiver).
pub struct NullReceiver;

impl CodeReceiver for NullReceiver {
    fn port(&self) -> u16 {
        0
    }

    fn await_query(&self) -> io::Result<String> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "device flow uses no loopback receiver",
        ))
    }
}

// The XdgCredStore IO invariants (atomic write, 0600 file, 0700 dir, round-trip) —
// a child module so it can root the real store at a private `dir` (bl-5b5a).
#[cfg(test)]
mod tests;
