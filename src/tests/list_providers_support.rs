//! Shared harness for the `bz --list-providers` tests (config §6.1): the driver that
//! runs `crate::list_providers` against the in-memory seams, the captured-outcome
//! struct, and the column reader. A subdir module, so cargo does not compile it as its
//! own test binary. There is no `Transport` double because the verb takes no
//! transport — offline is a property of `ProvidersIo`'s type (§6.1).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;

use crate::testing::MemoryCredStore;
use crate::{list_providers, Args, CredStore, EnvSnapshot, ProvidersIo};

pub struct Out {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// Drive the verb against an in-memory store and an explicit env, capturing both
/// streams. `env` names the config file (`BRAZEN_CONFIG`) and any `BRAZEN_*` scalar.
pub fn go(argv: &[&str], env: &[(&str, &str)], store: &dyn CredStore) -> Out {
    let mut out = Vec::new();
    let (code, stderr) = go_into(argv, env, store, &mut out);
    Out {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr,
    }
}

pub fn go_into(
    argv: &[&str],
    env: &[(&str, &str)],
    store: &dyn CredStore,
    out: &mut dyn Write,
) -> (u8, String) {
    let args = Args {
        argv: argv.iter().map(|s| (*s).to_string()).collect(),
        env: EnvSnapshot(
            env.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<BTreeMap<_, _>>(),
        ),
        tty: false,
        stdout_tty: false,
    };
    let mut err = Vec::new();
    let code = {
        let mut io = ProvidersIo {
            stdout: out,
            stderr: &mut err,
            store,
        };
        list_providers(&args, &mut io)
    };
    (code, String::from_utf8_lossy(&err).into_owned())
}

/// No config file: the listing is exactly the embedded floor (`data/defaults.toml`).
pub fn floor(store: &dyn CredStore) -> Out {
    go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        store,
    )
}

/// The column values of one named row, whitespace-split.
pub fn row<'a>(out: &'a str, name: &str) -> Vec<&'a str> {
    out.lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_default()
        .split_whitespace()
        .collect()
}

/// The floor listing under an arbitrary argv — the probe/usage cases.
pub fn floor_argv(argv: &[&str]) -> Out {
    go(
        argv,
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &MemoryCredStore::new(),
    )
}
