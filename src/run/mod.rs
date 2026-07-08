//! The `run` spine (arch §1, §4.4) — the whole binary behind one signature, pure
//! relative to its injected seams (`Transport`/`CredStore`/`ModelCache`/`Clock` + the
//! three writers). Two phases divided by the one boundary that matters: BEFORE the sink
//! exists, a failure is fatal and can only reach `stderr` (flag parse → 64,
//! input-open → 66, malformed config → 78); AFTER it, every failure is an in-band
//! `Event::Error` through the same sink, then the one `End`, then the exit (§5.9,
//! §8). This module owns the pre-sink phase and then dispatches the two independently-
//! toggled halves of the `--raw` directional split (§5.4, §13.14): a REQUEST half
//! (`generate::send_encoded` for a constructed request, or `serve::send_raw` for a
//! verbatim body) yields one prepared response, which the RESPONSE half — chosen by
//! `raw_out` in `drive` — projects as canonical events through `pump` or as verbatim
//! bytes through the `RawSink`. The typed core `generate` (request → encode → auth →
//! send → canonical events) is the normalized-in/canonical-out composition of those
//! halves; the response-decode primitives live in `events`, the seam in `drive`.

mod count;
mod discovery;
mod drive;
mod events;
mod generate;
mod models;
mod serve;

pub use count::{count_tokens, CountIo};
pub(crate) use discovery::{emit, HELP, VERSION_LINE};
pub use generate::generate;
/// The pure model-discovery request-shape helper (model-discovery §3.2) — exposed for
/// the override table tests; the data plane reaches it internally via `fetch_models`.
#[cfg(test)]
pub(crate) use models::models_req;
pub use models::{list_models, ListIo};

use std::io::{self, Read, Write};

use crate::canonical::{CanonicalError, ExitClass};
use crate::cli::{parse_args, Args};
use crate::config::partial::OutMode;
use crate::config::{config_path, defaults, partial_from_env, read_config_file};
use crate::pipeline::{
    open_input, pump, read_files, read_request, NdjsonSink, PrettySink, RawSink, Sink, Style,
    TextSink,
};
use crate::store::{Clock, CredStore, ModelCache};
use crate::transport::{Bytes, Transport};

/// The four impure data-plane seams, bundled (arch §1, §6.5) — the sibling of the
/// verbs' `ListIo`/`LoginIo` IO bundles. Every round-trip the generation path makes
/// goes through exactly these: the `Transport` (the one `ureq` user), the
/// credential store, the model cache, and the clock (auth-refresh expiry). The
/// writers stay separate from the `Host` because `run` borrows `stdout`/`stderr`
/// mutably AND simultaneously when it builds the sink — a seam reference is shared,
/// a writer reference is exclusive, so they cannot live in one struct.
pub struct Host<'a> {
    pub transport: &'a dyn Transport,
    pub store: &'a dyn CredStore,
    pub cache: &'a dyn ModelCache,
    pub clock: &'a dyn Clock,
}

/// The binary in one call (arch §1). Resolves config, reads the request (positional
/// XOR stdin), encodes, authenticates, sends one round-trip, decodes the framed
/// response into canonical events, and projects them through the mode's sink —
/// returning the POSIX exit code (`main` materializes the `ExitCode`).
pub fn run(
    args: Args,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    host: &Host,
) -> u8 {
    // ---- pre-sink: fatal, stderr-only (§5.9) ----
    let mut flags = match parse_args(&args.argv) {
        Ok(f) => f,
        Err(e) => return fail_early(stderr, e),
    };
    // The discovery short-circuits (§5.5): self-describing output to stdout, exit 0,
    // BEFORE any config/network — a probe must answer even with a broken config or no
    // provider. `--help` wins over `--version` (both is "show me everything").
    if flags.help {
        return emit(stdout, HELP);
    }
    if flags.version {
        return emit(stdout, VERSION_LINE);
    }
    let env = &args.env;
    let cfg_path = config_path(flags.config_path.take(), env);
    let file = match read_config_file(&cfg_path) {
        Ok(p) => p,
        Err(e) => return fail_early(stderr, e),
    };
    if flags.dump_config {
        return discovery::dump(stdout, stderr, flags.config, env, file);
    }
    // Friendly bare invocation (§5.5): an interactive terminal with no request source
    // — no positional prompt, no `--input FILE`, and stdin is a tty (so no piped
    // request either) — has nothing to read and would otherwise hit an empty-stdin
    // parse error. Print the usage to STDERR and exit 64. A pipe (`tty == false`) is
    // untouched: `echo '{…}' | bz` still reads and parses exactly as before.
    if args.tty && flags.prompt.is_none() && flags.input.is_none() && flags.files.is_empty() {
        let _ = stderr.write_all(HELP.as_bytes());
        return ExitClass::Usage.code();
    }
    let env_partial = match partial_from_env(env) {
        Ok(p) => p,
        Err(e) => return fail_early(stderr, e.into()),
    };
    let merged = flags.config.or(env_partial).or(file).or(defaults());
    let output = merged.output.unwrap_or(OutMode::Text);
    let thinking = merged.thinking.unwrap_or(false);
    // The two rawness axes toggle independently (§5.4, §13.14): `raw_out` is the OUTPUT
    // mode (the `RawSink`, `output == Raw`), `raw_in` the REQUEST half (send the stdin
    // body verbatim, skip the constructor+encode). Bare `--raw`/`--raw=both` leaves the
    // input axis to DERIVE from `output` — so it is BOTH, and a later `--json` lapses it
    // (§5.10.2); `--raw=in`/`--raw=out` pin it explicitly. A config/file `output = "raw"`
    // carries no direction, so it means BOTH — exactly `raw_in.unwrap_or(raw_out)`.
    let raw_out = output == OutMode::Raw;
    let raw_in = merged.raw_in.unwrap_or(raw_out);

    // `-f` is a constructor input; a verbatim request body (`--raw`/`--raw=in`) runs no
    // constructor, so the two cannot combine (§5.5) — a pre-sink usage refusal (64). The
    // refusal keys on `raw_in`, not `raw_out`: `--raw=out` still runs the constructor.
    if raw_in && !flags.files.is_empty() {
        let _ = writeln!(stderr, "--file cannot be combined with --raw");
        return ExitClass::Usage.code();
    }

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

    // `-f` attachments → ordered text parts, read pre-sink so a missing/unreadable/
    // non-UTF-8 file is the last stderr-only fatal (66), like the `--input` open (§5.5).
    let file_parts = match read_files(&flags.files) {
        Ok(parts) => parts,
        Err((path, e)) => {
            let _ = writeln!(stderr, "cannot read --file `{}`: {e}", path.display());
            return ExitClass::NoInput.code();
        }
    };

    // ---- the sink exists from here: every failure is in-band (§8) ----
    // The interactive skin is a tty-only choice WITHIN text mode (interactive-output
    // §3): `Style::resolve` owns the predicate, the shim feeds only `args.stdout_tty`.
    // A pretty resolve picks `PrettySink`; everything else is the literal prior path.
    let mut sink: Box<dyn Sink + '_> = match output {
        OutMode::Text => match Style::resolve(args.stdout_tty, output, env) {
            style if style.is_pretty() => {
                Box::new(PrettySink::new(&mut *stdout, &mut *stderr, thinking, style))
            }
            _ => Box::new(TextSink::new(&mut *stdout, &mut *stderr, thinking)),
        },
        OutMode::Ndjson => Box::new(NdjsonSink::new(&mut *stdout)),
        OutMode::Raw => Box::new(RawSink::new(&mut *stdout)),
    };
    // The REQUEST half (§5.4, §13.14). `raw_in` sends the stdin bytes verbatim (no
    // constructor, the model cache bypassed); the response half is then chosen by
    // `raw_out` alone in `drive`, so `--raw`=both and `--raw=in` share this request half.
    if raw_in {
        let sent = serve::send_raw(reader, merged, host);
        // `now` is read AFTER the send so a `Retry-After` HTTP-date parses relative to the
        // response (§3.3); the raw-out response half ignores it.
        return drive::drive(sent, raw_out, &mut *sink, host.clock.now());
    }
    // Normalized in: the constructor parses the request, folds config on its model, encodes.
    // Pre-`Sent` fatals (a malformed request, an unresolvable config) are in-band (§5.9).
    let request = match read_request(flags.prompt.as_deref(), file_parts, reader) {
        Ok(r) => r,
        Err(e) => return events::fail_inband(&mut *sink, e),
    };
    let req_model = (!request.model.is_empty()).then(|| request.model.clone());
    let cfg = match merged.into_resolved(req_model.as_deref()) {
        Ok(c) => c,
        Err(e) => return events::fail_inband(&mut *sink, e.into()),
    };
    // Normalized in, RAW out (§5.4): the encode-observability window — an ergonomic
    // request (positional/`-f`/config fold/model cache/auth) encoded to the wire, but the
    // provider's EXACT response bytes streamed back verbatim. `send_encoded` is the same
    // request half `generate` runs; only the response half differs (the `RawSink`).
    if raw_out {
        let sent = generate::send_encoded(request, cfg, host);
        return drive::drive(sent, true, &mut *sink, host.clock.now());
    }
    // Normalized in, canonical out — the public typed core: `pump` the `generate` stream
    // (request → canonical events, terminated by the one `End`) into the sink (§1, §5.6).
    pump(generate(request, cfg, host), &mut *sink)
}

/// Write a pre-sink fatal error to stderr and return its exit code (§5.9).
fn fail_early(stderr: &mut dyn Write, err: CanonicalError) -> u8 {
    let _ = writeln!(stderr, "{}", err.message);
    err.exit_code()
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
