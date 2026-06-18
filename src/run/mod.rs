//! The `run` spine (arch §1, §4.4) — the whole binary behind one signature, pure
//! relative to its injected seams (`Transport`/`CredStore`/`Clock` + the three
//! writers). Two phases divided by the one boundary that matters: BEFORE the sink
//! exists, a failure is fatal and can only reach `stderr` (flag parse → 64,
//! input-open → 66, malformed config → 78); AFTER it, every failure is an in-band
//! `Event::Error` through the same sink, then the one `End`, then the exit (§5.9,
//! §8). This module owns the pre-sink phase; the request pipeline (read → encode →
//! auth → send) lives in [`serve`] and the response-driving half (frame → decode →
//! project) in [`respond`].

mod models;
mod respond;
mod serve;

pub use models::{list_models, ListIo};

use std::io::{self, Read, Write};

use crate::canonical::{CanonicalError, ExitClass};
use crate::cli::{parse_args, Args};
use crate::config::partial::OutMode;
use crate::config::{
    config_path, defaults, dump_config, partial_from_env, read_config_file, EnvSnapshot,
    PartialConfig,
};
use crate::pipeline::{open_input, NdjsonSink, RawSink, Sink, TextSink};
use crate::store::{Clock, CredStore};
use crate::transport::{Bytes, Transport};

/// The binary in one call (arch §1). Resolves config, reads the request (positional
/// XOR stdin), encodes, authenticates, sends one round-trip, decodes the framed
/// response into canonical events, and projects them through the mode's sink —
/// returning the POSIX exit code (`main` materializes the `ExitCode`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    args: Args,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    transport: &dyn Transport,
    store: &dyn CredStore,
    clock: &dyn Clock,
) -> u8 {
    // ---- pre-sink: fatal, stderr-only (§5.9) ----
    let mut flags = match parse_args(&args.argv) {
        Ok(f) => f,
        Err(e) => return fail_early(stderr, e),
    };
    let env = &args.env;
    let cfg_path = config_path(flags.config_path.take(), env);
    let file = match read_config_file(&cfg_path) {
        Ok(p) => p,
        Err(e) => return fail_early(stderr, e),
    };
    if flags.dump_config {
        return dump(stdout, stderr, flags.config, env, file);
    }
    let env_partial = match partial_from_env(env) {
        Ok(p) => p,
        Err(e) => return fail_early(stderr, e.into()),
    };
    let merged = flags.config.or(env_partial).or(file).or(defaults());
    let output = merged.output.unwrap_or(OutMode::Text);
    let thinking = merged.thinking.unwrap_or(false);
    let raw = output == OutMode::Raw;

    // `--input FILE` is opened before the sink so its open failure is the last
    // stderr-only error (66); a real pipe is the injected `stdin` (§5.5).
    let mut input_file;
    let reader: &mut dyn Read = match &flags.input {
        Some(path) => match open_input(Some(path)) {
            Ok(f) => {
                input_file = f;
                &mut *input_file
            }
            Err(_) => {
                let _ = writeln!(stderr, "cannot open --input file `{}`", path.display());
                return ExitClass::NoInput.code();
            }
        },
        None => stdin,
    };

    // ---- the sink exists from here: every failure is in-band (§8) ----
    let mut sink: Box<dyn Sink + '_> = match output {
        OutMode::Text => Box::new(TextSink::new(&mut *stdout, &mut *stderr, thinking)),
        OutMode::Ndjson => Box::new(NdjsonSink::new(&mut *stdout)),
        OutMode::Raw => Box::new(RawSink::new(&mut *stdout)),
    };
    serve::serve(
        reader,
        raw,
        flags.prompt,
        merged,
        &mut *sink,
        transport,
        store,
        clock,
    )
}

/// Write a pre-sink fatal error to stderr and return its exit code (§5.9).
fn fail_early(stderr: &mut dyn Write, err: CanonicalError) -> u8 {
    let _ = writeln!(stderr, "{}", err.message);
    err.exit_code()
}

/// `--dump-config` (config §6): resolve the layers minus defaults, print the TOML
/// to stdout, exit 0. A bad env scalar surfaces as 78 on stderr (the same dump
/// re-runs the env projection, where the failure is reachable).
fn dump(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
) -> u8 {
    match dump_config(flags, env, file) {
        Ok(toml) => match stdout
            .write_all(toml.as_bytes())
            .and_then(|()| stdout.flush())
        {
            Ok(()) => ExitClass::Ok.code(),
            Err(io) => ExitClass::from_io(&io).code(),
        },
        Err(e) => fail_early(stderr, e.into()),
    }
}

/// Collect a response body iterator to end — the ONE home for draining a whole
/// body, shared by [`respond`]'s 2xx/error folds and [`models`]'s GET (both drain a
/// small complete document the framers never cut, not a stream). The mid-collection
/// transport drop rides up as the `io::Error` it arrived as, so each caller carries
/// the fact its own way: `respond` to an in-band `Transport` event, `models` into
/// its `CanonicalError` message (the carried `{e}`).
fn drain(body: Box<dyn Iterator<Item = io::Result<Bytes>>>) -> Result<Vec<u8>, io::Error> {
    let mut buf = Vec::new();
    for chunk in body {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf)
}
