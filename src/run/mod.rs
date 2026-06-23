//! The `run` spine (arch §1, §4.4) — the whole binary behind one signature, pure
//! relative to its injected seams (`Transport`/`CredStore`/`ModelCache`/`Clock` + the
//! three writers). Two phases divided by the one boundary that matters: BEFORE the sink
//! exists, a failure is fatal and can only reach `stderr` (flag parse → 64,
//! input-open → 66, malformed config → 78); AFTER it, every failure is an in-band
//! `Event::Error` through the same sink, then the one `End`, then the exit (§5.9,
//! §8). This module owns the pre-sink phase; the request pipeline (read → encode →
//! auth → send) lives in `serve` and the response-driving half (frame → decode →
//! project) in `respond`.

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
use crate::pipeline::{open_input, NdjsonSink, PrettySink, RawSink, Sink, Style, TextSink};
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
        return dump(stdout, stderr, flags.config, env, file);
    }
    // Friendly bare invocation (§5.5): an interactive terminal with no request source
    // — no positional prompt, no `--input FILE`, and stdin is a tty (so no piped
    // request either) — has nothing to read and would otherwise hit an empty-stdin
    // parse error. Print the usage to STDERR and exit 64. A pipe (`tty == false`) is
    // untouched: `echo '{…}' | bz` still reads and parses exactly as before.
    if args.tty && flags.prompt.is_none() && flags.input.is_none() {
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
    serve::serve(reader, raw, flags.prompt, merged, &mut *sink, host)
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

/// Print a fixed discovery document (`--help` / `--version`) to stdout, exit 0 —
/// the shared write-and-flush of the two self-describing short-circuits (§5.5),
/// mirroring [`dump`]'s stdout half: a broken stdout maps through `from_io` (so
/// `--help | head` is SIGPIPE/141, never a silent 0). `pub(crate)` so the control
/// verbs (`list-models`, `login`) honor the SAME short-circuit with the SAME doc —
/// one help screen, not three.
pub(crate) fn emit(stdout: &mut dyn Write, doc: &str) -> u8 {
    match stdout
        .write_all(doc.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitClass::Ok.code(),
        Err(io) => ExitClass::from_io(&io).code(),
    }
}

/// The `--version` line: the package version (Cargo's, the single source) + newline.
/// `pub(crate)` so every verb's `--version` prints the one line (see [`emit`]).
pub(crate) const VERSION_LINE: &str = concat!("bz ", env!("CARGO_PKG_VERSION"), "\n");

/// The `--help` document and the friendly bare-invocation hint (§5.5): one screen —
/// synopsis, the input model (positional prompt XOR a canonical request on stdin),
/// the two control verbs, the flag list, and the exit-code table (§8). Kept tight
/// and POSIX-conventional; the single source for EVERY verb's `--help` stdout
/// (`run`, `list-models`, `login`), the bare-on-tty stderr usage, and `login`'s usage.
pub(crate) const HELP: &str = concat!(
    "bz ",
    env!("CARGO_PKG_VERSION"),
    " — a stateless LLM adapter: one request, one round-trip, one POSIX exit.\n",
    "\n",
    "USAGE:\n",
    "    bz [FLAGS] \"PROMPT\"        one-shot: the positional prompt is the request\n",
    "    echo '{…}' | bz [FLAGS]    pipe a canonical request (JSON) on stdin instead\n",
    "    bz <VERB> [ARGS]           a control verb (below)\n",
    "\n",
    "The request arrives exactly one way: a positional PROMPT (argv) XOR a canonical\n",
    "request on stdin. A prompt wins and stdin is not read. Output is a projection\n",
    "chosen by flag; the default is plain text.\n",
    "\n",
    "VERBS:\n",
    "    login <provider> [--browser]\n",
    "                         obtain and store an OAuth/SSO credential (the one\n",
    "                         interactive surface; never entered by the data plane).\n",
    "                         Default: the headless device flow (shows a code to enter\n",
    "                         on another device). --browser: the loopback browser flow\n",
    "                         (opens a URL, captures the redirect) when the provider's\n",
    "                         registered redirect is a loopback URL.\n",
    "    list-models          one GET: list the resolved provider's models\n",
    "\n",
    "FLAGS:\n",
    "    --provider <id>      provider row id (else routed from the model)\n",
    "    --model <id>         model id; a partial/absent id resolves against the cache\n",
    "    --api-key <key>      inline credential (else the credential store / env)\n",
    "    --system <text>      leading system prompt\n",
    "    --max-tokens <n>     generation cap\n",
    "    --temperature <f>    sampling temperature\n",
    "    --top-p <f>          nucleus sampling\n",
    "    --stream/--no-stream stream the response (default) or fold one JSON body\n",
    "    --thinking           include reasoning/thinking output (text mode)\n",
    "    --text               human-readable text (default)\n",
    "    --json               the full NDJSON canonical event stream\n",
    "    --raw                pass bytes through verbatim, provider-native both ways\n",
    "    --input <file>       read the request from a file instead of stdin\n",
    "    --config <file>      use this config file (else the default search path)\n",
    "    --timeout-connect <s> / --timeout-response <s> / --timeout-idle <s>\n",
    "    --dump-config        print the merged config as TOML, exit 0\n",
    "    --help, -h           print this help, exit 0\n",
    "    --version, -V        print the version, exit 0\n",
    "\n",
    "EXIT CODES (sysexits):\n",
    "    0    success (incl. a provider refusal — a 200)\n",
    "    64   usage: bad/unknown flag, malformed stdin request\n",
    "    66   --input file missing or unreadable\n",
    "    69   transport error, upstream 4xx (incl. 429), premature EOF\n",
    "    70   upstream 5xx (retryable)\n",
    "    77   auth: 401/403, missing credentials, login/refresh failure\n",
    "    78   config: no/unknown/ambiguous provider or model, bad config\n",
    "    130/141/143  interrupted by signal (SIGINT/SIGPIPE/SIGTERM)\n",
);

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
