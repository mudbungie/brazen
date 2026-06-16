//! `bz` — the brazen binary entry point (arch §1, §9.5, §10).
//!
//! The thin shim: restore SIGPIPE, snapshot the real argv/env, wire the native
//! impure impls behind their seams, call `brazen::run`, and materialize its `u8`
//! into a `process::ExitCode`. This is the ONE file excluded from coverage
//! (Makefile `cov` `--ignore-filename-regex`): every native impurity — the
//! network, the credential file, the system clock, the SIGPIPE syscall — lives
//! here so the library reaches 100% behind injection. `HttpTransport`'s real
//! rustls body lands in its own task (bl-838c); until then it is wired but inert.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use brazen::{Args, Cred, CredStore, EnvSnapshot};
use brazen::{CanonicalError, ErrorKind};
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
    let transport = HttpTransport;
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

/// The real network seam (arch §4.1, §9.1, §10) — a blocking, rustls-backed HTTP
/// round-trip. Its body lands in bl-838c; until then it is wired but returns a
/// `Transport` error so the binary links and the seam is real (the run spine and
/// every pure stage are proven end-to-end against `MockTransport`).
struct HttpTransport;

impl Transport for HttpTransport {
    fn send(&self, _wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        Err(CanonicalError {
            kind: ErrorKind::Transport,
            message: "HTTP transport not yet implemented (bl-838c)".to_owned(),
            provider_detail: None,
        })
    }
}
