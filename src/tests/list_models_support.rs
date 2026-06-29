//! Shared harness for the `bz --list-models` control-flag tests (model-discovery §2): the
//! anthropic `/v1/models` body, the captured-outcome struct, and the drivers that run
//! `crate::list_models` against the in-memory seams. A subdir module, so cargo does
//! not compile it as its own test binary. `go_env` carries an `EnvSnapshot` so a test
//! can inject `BRAZEN_OUTPUT` (the resolved-`OutMode` path the verb shares with `run`).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::testing::{FakeClock, MemoryModelCache};
use crate::{list_models, Args, CredStore, EnvSnapshot, ListIo, ModelCache, Transport};

/// The common verb argv: list anthropic's models behind an inline key. Shared so the
/// repeated `--list-models --provider anthropic --api-key sk` stays one token, not a
/// fmt-expanded seven-line array in every test (keeps the file under the 300-line cap).
pub const ANT: &[&str] = &[
    "--list-models",
    "--provider",
    "anthropic",
    "--api-key",
    "sk",
];

/// The anthropic `/v1/models` body (newest-first), as `data[].id` (§3.1).
pub const MODELS: &[u8] = br#"{"data":[
    {"type":"model","id":"claude-opus-4-1-20250805"},
    {"type":"model","id":"claude-sonnet-4-5-20250929"}
],"has_more":false}"#;

/// Outcome of one `list-models`: exit code, captured stdout, captured stderr.
pub struct Out {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// Drive `crate::list_models` against the in-memory seams. The argv carries the
/// `--list-models` control flag (the entry re-parses the WHOLE argv authoritatively).
pub fn go(argv: &[&str], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    go_env(argv, &EnvSnapshot(BTreeMap::new()), tx, store)
}

/// Drive the verb with an explicit `EnvSnapshot` (e.g. `BRAZEN_OUTPUT=ndjson`) — the
/// resolved-`OutMode` path the verb shares with the data plane.
pub fn go_env(argv: &[&str], env: &EnvSnapshot, tx: &dyn Transport, store: &dyn CredStore) -> Out {
    let mut out = Vec::new();
    let code = go_out(argv, env, tx, store, &mut out);
    Out {
        code: code.0,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: code.1,
    }
}

/// Drive the verb against an arbitrary stdout writer (e.g. a failing one) and an
/// explicit env, returning the exit code and captured stderr. A throwaway cache backs
/// the `put`; tests asserting the write use [`go_cache`] to read it back.
pub fn go_out(
    argv: &[&str],
    env: &EnvSnapshot,
    tx: &dyn Transport,
    store: &dyn CredStore,
    out: &mut dyn Write,
) -> (u8, String) {
    go_into(argv, env, tx, store, out, &MemoryModelCache::new())
}

/// Drive the verb and return the `MemoryModelCache` it wrote, so a test asserts the
/// verb's SOLE cache write (model-discovery §5) — the `put` of the decoded list.
pub fn go_cache(
    argv: &[&str],
    tx: &dyn Transport,
    store: &dyn CredStore,
) -> (Out, MemoryModelCache) {
    let cache = MemoryModelCache::new();
    let mut out = Vec::new();
    let (code, stderr) = go_into(
        argv,
        &EnvSnapshot(BTreeMap::new()),
        tx,
        store,
        &mut out,
        &cache,
    );
    (
        Out {
            code,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr,
        },
        cache,
    )
}

/// The one driver: run `list_models` against the given seams + an explicit `cache`.
fn go_into(
    argv: &[&str],
    env: &EnvSnapshot,
    tx: &dyn Transport,
    store: &dyn CredStore,
    out: &mut dyn Write,
    cache: &dyn ModelCache,
) -> (u8, String) {
    let args = Args {
        argv: argv.iter().map(|s| (*s).to_string()).collect(),
        env: env.clone(),
        tty: false,
        stdout_tty: false,
    };
    let clock = FakeClock::new(0);
    let mut err = Vec::new();
    let code = {
        let mut io = ListIo {
            stdout: out,
            stderr: &mut err,
            transport: tx,
            store,
            cache,
            clock: &clock,
        };
        list_models(&args, &mut io)
    };
    (code, String::from_utf8_lossy(&err).into_owned())
}

/// A stdout writer that always fails — the listing write-failure path (→69).
pub struct FailWriter;
impl Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}
