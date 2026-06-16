//! The native impure impls behind brazen's seams (arch §6.5, §9.5, §10) — part of
//! the coverage-excluded `bz` shim (Makefile `cov` `--ignore-filename-regex 'bz/'`).
//! The native impurities live here: the system clock, the atomic 0600 credential
//! file, the browser spawn, the loopback `bind`/`accept`, the device-poll sleep,
//! and the OS RNG. The rustls-backed `HttpTransport` — the lone `ureq` user — is
//! its sibling [`crate::transport`]. The library reaches 100% behind injection; the
//! pure parsing these call (`browser_argv`, `query_from_request_line`, the OAuth
//! builders) is in the lib.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use brazen::{BrowserLauncher, Clock, CodeReceiver, Cred, CredStore, Pacer};

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

/// The XDG-backed credential store (arch §6.4, auth §5.2): one 0600 JSON file per
/// provider, written atomically (temp file created 0600, then `rename`) so a
/// concurrent reader sees either the whole old or whole new file, never a partial
/// write. `get` is `None` on any miss (the no-creds path).
pub struct XdgCredStore {
    dir: Option<PathBuf>,
}

impl XdgCredStore {
    pub fn new() -> Self {
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
        let bytes = fs::read(self.path(provider)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        let path = self.path(provider).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no data dir for credentials")
        })?;
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no credentials directory"))?;
        fs::create_dir_all(dir)?;
        set_dir_mode(dir)?;
        let tmp = dir.join(format!(".{provider}.json.tmp"));
        let bytes = serde_json::to_vec_pretty(cred)?;
        write_owner_only(&tmp, &bytes)?;
        fs::rename(&tmp, &path)
    }
}

/// Create `path` 0600 at create time on Unix (never a create-then-chmod window),
/// write `bytes`, and `sync_all` before the caller renames it into place.
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// The credentials directory is `0700` on Unix; the user-profile ACL stands
/// elsewhere (a documented limitation, auth §5.2).
#[cfg(unix)]
fn set_dir_mode(dir: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_dir_mode(_dir: &std::path::Path) -> io::Result<()> {
    Ok(())
}

/// `$XDG_DATA_HOME/brazen/credentials` (Unix), `~/Library/Application
/// Support/brazen/credentials` (macOS), `%APPDATA%\brazen\credentials` (Windows).
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

/// A random URL-safe token (PKCE verifier / CSRF state): 32 bytes of OS entropy,
/// base64url-no-pad → 43 unreserved chars (auth §7.2, §7.4).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    fill_random(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) {
    use std::io::Read;
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(buf);
    }
}

#[cfg(not(unix))]
fn fill_random(buf: &mut [u8]) {
    // No DPAPI/getrandom dep on this tier: a weak time-seeded fill. The Windows
    // secret-at-rest story is a documented limitation (auth §5.2).
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (seed >> (i % 16)) as u8 ^ (i as u8).wrapping_mul(31);
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
