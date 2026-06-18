//! Shared harness for the `bz list-models` verb tests (model-discovery §2): the
//! anthropic `/v1/models` body, the captured-outcome struct, and the drivers that run
//! `brazen::list_models` against the in-memory seams. A subdir module, so cargo does
//! not compile it as its own test binary. `go_env` carries an `EnvSnapshot` so a test
//! can inject `BRAZEN_OUTPUT` (the resolved-`OutMode` path the verb shares with `run`).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{self, Write};

use brazen::testing::FakeClock;
use brazen::{list_models, Args, CredStore, EnvSnapshot, ListIo, Transport};

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

/// Drive `brazen::list_models` against the in-memory seams. The argv begins with the
/// `list-models` verb word (the shim strips none — the verb parses `argv[1..]`).
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
/// explicit env, returning the exit code and captured stderr.
pub fn go_out(
    argv: &[&str],
    env: &EnvSnapshot,
    tx: &dyn Transport,
    store: &dyn CredStore,
    out: &mut dyn Write,
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
