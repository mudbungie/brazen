//! Shared harness for the end-to-end `run` tests (arch §9.6): one place to build
//! `Args`, drive `run` against the in-memory seams, and hold the golden fixtures +
//! test doubles. A subdirectory module, so cargo does not compile it as its own
//! test binary; `#![allow(dead_code)]` because each split test crate uses only a
//! subset of these helpers.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brazen::testing::{FakeClock, MemoryModelCache, MockTransport};
use brazen::{run, Args, CanonicalError, CredStore, EnvSnapshot, ErrorKind, Host, ModelCache};
use brazen::{Transport, TransportResponse, WireRequest};

pub const BASIC: &[u8] = include_bytes!("../fixtures/anthropic_messages_basic.sse");
pub const REFUSAL: &[u8] = include_bytes!("../fixtures/anthropic_messages_refusal.sse");
pub const OVERLOADED: &[u8] = include_bytes!("../fixtures/anthropic_error_overloaded.json");

// A complete message_start frame with NO terminator — premature EOF (terminated
// stays false → exit 69).
pub const TRUNCATED: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"role\":\"assistant\",\"model\":\"x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";

// message_start (blank-line terminated) then message_stop with NO trailing blank
// line — `finish` must flush the buffered stop frame, which sets `terminated`.
pub const FINISH_FLUSH: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"role\":\"assistant\",\"model\":\"x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n";

/// Outcome of one `run`: exit code, captured stdout, captured stderr.
pub struct Out {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// Build `Args` from literal argv + env pairs. `tty` is `false` — a piped/scripted
/// stdin, the common test shape; the bare-on-tty path drives `go_tty` instead.
pub fn args(argv: &[&str], env: &[(&str, &str)]) -> Args {
    Args {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: EnvSnapshot(
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        ),
        tty: false,
        stdout_tty: false,
    }
}

/// Drive `run` with `args.tty = true` and an empty stdin — the interactive-terminal
/// shape the `bz` shim injects (an empty reader for a tty, §5.5). The bare-invocation
/// usage path (no prompt, no `--input`) is reachable only here.
pub fn go_tty(argv: &[&str], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    let mut a = args(argv, &[]);
    a.tty = true;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let clock = FakeClock::new(0);
    let cache = MemoryModelCache::new();
    let host = host(tx, store, &cache, &clock);
    let code = run(a, &mut Cursor::new(Vec::new()), &mut out, &mut err, &host);
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    }
}

/// Drive `run` with `args.stdout_tty = true` and a pretty-enabling env (a real TERM
/// and a UTF-8 locale) — the interactive-terminal shape (interactive-output §3). Picks
/// the `PrettySink` in the text arm, so the end-to-end pretty path is reachable. Stdin
/// is empty (a positional prompt), so only the projection differs from `go`.
pub fn go_pretty(argv: &[&str], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    let mut a = args(argv, &[("TERM", "xterm-256color"), ("LANG", "en_US.UTF-8")]);
    a.stdout_tty = true;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let clock = FakeClock::new(0);
    let cache = MemoryModelCache::new();
    let host = host(tx, store, &cache, &clock);
    let code = run(a, &mut Cursor::new(Vec::new()), &mut out, &mut err, &host);
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    }
}

/// Drive `run` with a byte-slice stdin (the common case) against an EMPTY cache — so a
/// full model id resolves to itself verbatim (`select_model` total, §4), byte-for-byte
/// the pre-cache behavior the existing fixtures assert.
pub fn go(
    argv: &[&str],
    env: &[(&str, &str)],
    stdin: &[u8],
    tx: &dyn Transport,
    store: &dyn CredStore,
) -> Out {
    go_reader(argv, env, &mut Cursor::new(stdin.to_vec()), tx, store)
}

/// Drive `run` with an arbitrary stdin reader (e.g. a failing reader), empty cache.
pub fn go_reader(
    argv: &[&str],
    env: &[(&str, &str)],
    reader: &mut dyn Read,
    tx: &dyn Transport,
    store: &dyn CredStore,
) -> Out {
    go_cached(argv, env, reader, tx, store, &MemoryModelCache::new())
}

/// Drive `run` with an explicit `cache` — the serve cache-lookup tests prime it so a
/// partial seed expands to a wire id (and assert the SINGLE generation send, no probe).
pub fn go_cached(
    argv: &[&str],
    env: &[(&str, &str)],
    reader: &mut dyn Read,
    tx: &dyn Transport,
    store: &dyn CredStore,
    cache: &dyn ModelCache,
) -> Out {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let clock = FakeClock::new(0);
    let host = host(tx, store, cache, &clock);
    let code = run(args(argv, env), reader, &mut out, &mut err, &host);
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    }
}

/// Drive `run` with a stdout that always fails `BrokenPipe` — the SIGPIPE→141
/// path. Empty stdin + the happy transport, so only the write failure shapes it.
pub fn run_broken_pipe(argv: &[&str], store: &dyn CredStore) -> u8 {
    let mut out = BrokenPipeWriter;
    let mut err = Vec::new();
    let clock = FakeClock::new(0);
    let cache = MemoryModelCache::new();
    let tx = ok_basic();
    let host = host(&tx, store, &cache, &clock);
    run(
        args(argv, &[]),
        &mut Cursor::new(Vec::new()),
        &mut out,
        &mut err,
        &host,
    )
}

/// Bundle the four in-memory seams into a [`Host`] — the data-plane injection point
/// every `run` driver shares (mirrors the `bz` shim's own `Host` wiring).
fn host<'a>(
    tx: &'a dyn Transport,
    store: &'a dyn CredStore,
    cache: &'a dyn ModelCache,
    clock: &'a FakeClock,
) -> Host<'a> {
    Host {
        transport: tx,
        store,
        cache,
        clock,
    }
}

/// The common happy case: an anthropic stream behind an inline api-key (no store).
pub fn ok_basic() -> MockTransport {
    MockTransport::ok(vec![BASIC])
}

pub fn empty_store() -> brazen::testing::MemoryCredStore {
    brazen::testing::MemoryCredStore::new()
}

/// A transport whose handshake fails (connect/DNS/TLS class) — exit 69.
pub struct ErrTransport;
impl Transport for ErrTransport {
    fn send(&self, _: WireRequest) -> Result<TransportResponse, CanonicalError> {
        Err(CanonicalError {
            kind: ErrorKind::Transport,
            message: "connection refused".into(),
            provider_detail: None,
        })
    }
}

/// A writer that always fails with `BrokenPipe` — the Windows SIGPIPE path (141).
pub struct BrokenPipeWriter;
impl Write for BrokenPipeWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

/// A reader that always fails — the raw-body read-error path (64).
pub struct FailReader;
impl Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}

/// A self-deleting temp file for config-file tests.
pub struct TempFile(pub PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn temp(contents: &str) -> TempFile {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("brazen_run_{}_{}.toml", std::process::id(), n));
    fs::write(&path, contents).unwrap();
    TempFile(path)
}
