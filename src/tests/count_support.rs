//! Shared harness for the `bz --count-tokens` control-flag tests (architecture §5.10.1,
//! anthropic-messages §2.11, providers §10.1): the drivers that run `crate::count_tokens`
//! against the in-memory seams, plus the captured-outcome struct and a failing stdout
//! writer. A subdir module, so cargo does not compile it as its own test binary.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{self, Cursor, Read, Write};

use crate::testing::{FakeClock, MemoryModelCache};
use crate::{count_tokens, Args, CountIo, CredStore, EnvSnapshot, Transport};

/// A canonical request naming a full model id (verbatim against the empty cache, §4) —
/// the common count input, behind an inline api-key so a run touches no store.
pub const REQ: &[u8] = br#"{"model":"claude-x","messages":[{"role":"user","content":"hi"}]}"#;

/// The common count argv: count anthropic's tokens behind an inline key.
pub const ANT: &[&str] = &[
    "--count-tokens",
    "--provider",
    "anthropic",
    "--api-key",
    "sk",
];

/// A 2xx anthropic count body.
pub const COUNT: &[u8] = br#"{"input_tokens":42}"#;

/// Outcome of one `count-tokens`: exit code, captured stdout, captured stderr.
pub struct Out {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// Drive `crate::count_tokens` with a byte-slice stdin against an EMPTY cache.
pub fn go(argv: &[&str], stdin: &[u8], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    go_env(argv, &[], stdin, tx, store)
}

/// Drive the op with an explicit env (e.g. `BRAZEN_OUTPUT=ndjson`) — the resolved-`OutMode`
/// path the op shares with the data plane and `--list-models`.
pub fn go_env(
    argv: &[&str],
    env: &[(&str, &str)],
    stdin: &[u8],
    tx: &dyn Transport,
    store: &dyn CredStore,
) -> Out {
    let mut out = Vec::new();
    let (code, stderr) = go_out(
        argv,
        env,
        &mut Cursor::new(stdin.to_vec()),
        tx,
        store,
        &mut out,
    );
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr,
    }
}

/// Drive the op with an arbitrary stdin reader (a failing reader, or `--input`'s empty
/// reader) — returning the code + captured stderr against a throwaway stdout.
pub fn go_reader(
    argv: &[&str],
    reader: &mut dyn Read,
    tx: &dyn Transport,
    store: &dyn CredStore,
) -> Out {
    let mut out = Vec::new();
    let (code, stderr) = go_out(argv, &[], reader, tx, store, &mut out);
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr,
    }
}

/// The one driver: run `count_tokens` against the given seams + an explicit stdout writer
/// (so a test can inject a failing one), an empty cache, and a fake clock.
pub fn go_out(
    argv: &[&str],
    env: &[(&str, &str)],
    reader: &mut dyn Read,
    tx: &dyn Transport,
    store: &dyn CredStore,
    out: &mut dyn Write,
) -> (u8, String) {
    let args = Args {
        argv: argv.iter().map(|s| (*s).to_string()).collect(),
        env: EnvSnapshot(
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        ),
        tty: false,
        stdout_tty: false,
    };
    let clock = FakeClock::new(0);
    let cache = MemoryModelCache::new();
    let mut err = Vec::new();
    let code = {
        let mut io = CountIo {
            stdout: out,
            stderr: &mut err,
            transport: tx,
            store,
            cache: &cache,
            clock: &clock,
        };
        count_tokens(&args, reader, &mut io)
    };
    (code, String::from_utf8_lossy(&err).into_owned())
}

/// A stdout writer that always fails — the count write-failure path (→69).
pub struct FailWriter;
impl Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}

/// A reader that always fails — the request-read error path (→64).
pub struct FailReader;
impl Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}
