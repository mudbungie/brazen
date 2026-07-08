//! Token counting (architecture §5.10.1, anthropic-messages §2.11, providers §10.1): the
//! `bz --count-tokens` control flag. Reads a canonical request the SAME way the data plane
//! does (a positional prompt XOR a canonical request on stdin/`--input`, plus `-f`),
//! resolves provider/model identically, does ONE round-trip to the provider's count
//! endpoint, and prints the `input_tokens`. A provider with no count endpoint DECLINES
//! (`Config`, 78) — a fabricated estimate is a lie, so the caller's own estimate stays its
//! fallback. This module is the VERB (parse, resolve, read, print); the wire half
//! (encode-count, auth, send, decode) lives in [`fetch`]. No retry, NO cache write.

use std::io::{Read, Write};

use crate::canonical::{CanonicalError, ExitClass};
use crate::cli::{parse_args, Args};
use crate::config::{config_path, defaults, partial_from_env, read_config_file, OutMode};
use crate::pipeline::{open_input, read_files, read_request};
use crate::store::{Clock, CredStore, ModelCache};
use crate::transport::Transport;

mod fetch;

use fetch::fetch_count;

/// The injected seams + writers for one `bz --count-tokens` — the sibling of `ListIo`,
/// with a `reader` (the count op CONSUMES a request, unlike the listing verb). Reuses the
/// data-plane `Transport`/`CredStore`/`ModelCache`/`Clock`: the model seed is placed
/// against the same cache (READ-only — no write), and the one round-trip goes through the
/// same `Auth::apply`. `stdout` gets the count; any error goes to `stderr`.
pub struct CountIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub transport: &'a dyn Transport,
    pub store: &'a dyn CredStore,
    pub cache: &'a dyn ModelCache,
    pub clock: &'a dyn Clock,
}

/// Run `bz --count-tokens` and return the POSIX exit code (§5.10.1). Reads the request
/// from `reader` (stdin / `--input`) or the positional prompt, resolves provider/model as
/// the data plane does, does ONE round-trip, and prints `{"input_tokens": N}` under
/// `--json` else the bare `N`. A failure is written to stderr and mapped to its exit
/// (usage 64 / no-input 66 / config 78 / auth 77 / non-2xx 69-70 / malformed body 70).
pub fn count_tokens(args: &Args, reader: &mut dyn Read, io: &mut CountIo) -> u8 {
    match run_count(args, reader, io) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_count(args: &Args, reader: &mut dyn Read, io: &mut CountIo) -> Result<u8, CanonicalError> {
    let flags = parse_args(&args.argv)?;
    // The discovery probes ride the SAME flag layer + doc (§5.5): self-describe and exit 0
    // BEFORE any config/network, so a probe answers even with no provider.
    if flags.help {
        return Ok(super::emit(io.stdout, super::HELP));
    }
    if flags.version {
        return Ok(super::emit(io.stdout, super::VERSION_LINE));
    }
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    // The output shape is the RESOLVED `OutMode` (flag/env/file), the same fact the data
    // plane and `--list-models` fold: Ndjson → the object, else the bare number.
    let json = merged.output == Some(OutMode::Ndjson);
    // Read the request the SAME way the data plane does (§5.5): `-f` attachments and the
    // `--input` open are pre-parse I/O, so a missing/unreadable file is `NoInput` (66)
    // written on stderr and returned directly (no `ErrorKind` maps to 66); `read_request`
    // then applies the positional-XOR-canonical rule (a malformed body is `ParseInput`/64).
    let file_parts = match read_files(&flags.files) {
        Ok(parts) => parts,
        Err((path, e)) => {
            let _ = writeln!(io.stderr, "cannot read --file `{}`: {e}", path.display());
            return Ok(ExitClass::NoInput.code());
        }
    };
    let mut input_file;
    let rdr: &mut dyn Read = match &flags.input {
        Some(path) => match open_input(Some(path)) {
            Ok(f) => {
                input_file = f;
                &mut *input_file
            }
            Err(_) => {
                let _ = writeln!(io.stderr, "cannot open --input file `{}`", path.display());
                return Ok(ExitClass::NoInput.code());
            }
        },
        None => reader,
    };
    let request = read_request(flags.prompt.as_deref(), file_parts, rdr)?;
    let req_model = (!request.model.is_empty()).then(|| request.model.clone());
    let cfg = merged.into_resolved(req_model.as_deref())?;
    let n = fetch_count(request, cfg, io)?;
    print_count(io.stdout, n, json).map_err(write_failed)?;
    Ok(0)
}

/// Print the token count: `--json`/`ndjson` the canonical `{"input_tokens": N}` object,
/// else the bare number. The key is ALWAYS `input_tokens` — the dialect's wire key
/// (`totalTokens` for Google) is a decode detail, never surfaced (anthropic-messages §2.11).
fn print_count(out: &mut dyn Write, n: u32, json: bool) -> std::io::Result<()> {
    if json {
        writeln!(out, "{}", serde_json::json!({ "input_tokens": n }))
    } else {
        writeln!(out, "{n}")
    }
}

/// A stdout write failure for the count → `Transport` (→69), the count analog of the
/// listing verb's write-failure path.
fn write_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: crate::canonical::ErrorKind::Transport,
        message: format!("failed to write token count: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
