//! `bz` — the brazen binary entry point (arch §1, §9.5, §10).
//!
//! The thin shim: restore SIGPIPE, snapshot the real argv/env, wire the native
//! impure impls behind their seams, call `brazen::run`, and materialize its `u8`
//! into a `process::ExitCode`. This is the ONE file excluded from coverage
//! (Makefile `cov` `--ignore-filename-regex`): every native impurity — the
//! network, the credential file, the system clock, the SIGPIPE syscall — lives
//! here so the library reaches 100% behind injection — the rustls-backed
//! `HttpTransport` is the one native impurity that can only be smoke-tested.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brazen::{Args, Cred, CredStore, EnvSnapshot};
use brazen::{Bytes, CanonicalError, ErrorKind};
use brazen::{Clock, Transport, TransportResponse, WireRequest};

fn main() -> ExitCode {
    restore_sigpipe();
    let args = Args {
        argv: std::env::args().skip(1).collect(),
        env: EnvSnapshot(std::env::vars().collect::<BTreeMap<_, _>>()),
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let transport = HttpTransport::new();
    let store = XdgCredStore::new();
    let clock = SystemClock;
    let code = brazen::run(
        args,
        &mut stdin.lock(),
        &mut stdout.lock(),
        &mut stderr.lock(),
        &transport,
        &store,
        &clock,
    );
    ExitCode::from(code)
}

/// Restore SIGPIPE to `SIG_DFL` (arch §5.8): Rust sets `SIG_IGN`, which would turn
/// a closed-stdout write into a `BrokenPipe` error instead of letting the kernel
/// kill us with the signal (exit 141, like `cat | head`). Windows has no SIGPIPE —
/// `pump` maps its `BrokenPipe` write error to the same 141 there.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: a single libc call at startup, before any thread is spawned; it only
    // resets a signal disposition. The lib forbids unsafe — this is the shim's.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// The system clock (arch §6.5): the lib's `Clock` seam, the one place real time
/// is read. A pre-1970 clock is clamped to 0 rather than panicking.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// The XDG-backed credential store (arch §6.4): one 0600 JSON file per provider
/// under the platform data dir. `get` is `None` on any miss (the no-creds path);
/// `put` writes atomically-enough for v0.1 (truncate + 0600). `bz login`'s
/// hardening (temp-file + rename) lands with OAuth (bl-3c36).
struct XdgCredStore {
    dir: Option<PathBuf>,
}

impl XdgCredStore {
    fn new() -> Self {
        XdgCredStore {
            dir: credentials_dir(),
        }
    }

    fn path(&self, provider: &str) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{provider}.json")))
    }
}

impl CredStore for XdgCredStore {
    fn get(&self, provider: &str) -> Option<Cred> {
        let path = self.path(provider)?;
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        let path = self.path(provider).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no data dir for credentials")
        })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(cred)?;
        fs::write(&path, bytes)?;
        set_owner_only(&path)
    }
}

/// `$XDG_DATA_HOME/brazen/credentials` (Unix), `~/Library/Application
/// Support/brazen/credentials` (macOS), `%APPDATA%\brazen\credentials` (Windows).
/// `None` when even the home dir is unknown — surfaced as the no-creds path.
fn credentials_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("brazen").join("credentials"))
}

#[cfg(target_os = "macos")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
}

#[cfg(target_os = "windows")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

/// Enforce 0600 on the secret file (arch §6.4). On non-Unix the user-profile ACL
/// stands — a documented limitation, not a code branch.
#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> io::Result<()> {
    Ok(())
}

/// The real network seam (arch §4.1, §9.1, §10): a blocking, rustls-backed HTTP
/// round-trip via one reused `ureq` agent (rustls + bundled `webpki-roots`, no
/// OpenSSL, no async runtime). `http_status_as_error(false)` lets a non-2xx come
/// back as an ordinary response so its status rides `TransportResponse.status`
/// (peeked even under `--raw`) and `decode` derives the `ErrorKind` from that one
/// authoritative value (arch §8) — the transport never guesses it back. Only the
/// connect handshake is bounded (connect + response-headers timeouts); the body
/// is left unbounded so a long stream is never truncated mid-generation.
struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    fn new() -> Self {
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
        // Anything short of a received HTTP response — connect/DNS/TLS/timeout —
        // is a `Transport` error (arch §8 → exit 69). A non-2xx is NOT an error
        // here (status disabled above): it flows on as a normal response.
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
