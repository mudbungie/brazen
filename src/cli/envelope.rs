//! The call envelope (arch §5.10.5): `bz --envelope` takes its COMMAND LINE as one
//! JSON object on stdin — `{"args": [string…], "stdin": string}` — for a caller that
//! can deliver a tool input but cannot type an argv (a remote tool executor whose
//! contract is "the input, as JSON, on the child's stdin"). `args` is what would have
//! followed `bz`; `stdin` is what would have been piped in.
//!
//! The sender is on another machine, so the command line is FILTERED: only the flags
//! in [`ALLOWED`] may be sent. Every other flag — anything naming a file or a
//! configuration, printing one, carrying a credential, attaching input, moving the
//! endpoint or starting the listener — and any positional prompt is refused (64)
//! before a byte of config is read or a seam is touched. What survives is handed back
//! to the shim as an ordinary `Args` + stdin, so everything after this layer IS the
//! direct run: same stdout, same stderr, same exit. The environment is not filtered —
//! it is the executing box's own.
//!
//! This sits BEFORE the grammar, not in it: `parse_args` does not know the flag, so an
//! envelope inside an envelope (or an embedder that skipped this layer) meets the
//! ordinary unknown-flag 64 with no branch of its own.

use std::io::{Read, Write};

use serde::Deserialize;

use crate::canonical::ExitClass;

use super::Args;

/// The mode flag. It stands alone: the command line it runs is the envelope's.
const FLAG: &str = "--envelope";

/// The closed set of flags a far caller may send — exactly those that spell the five
/// command lines a server sends (a model call, the provider table, the sign-in
/// catalog, a model listing, a sign-in). `--provider` alone takes a value.
const ALLOWED: [&str; 6] = [
    "--json",
    "--provider",
    "--list-providers",
    "--list-models",
    "--login",
    "--browser",
];

/// The envelope document. Closed: an unknown key is a malformed envelope, never a
/// silently dropped instruction.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    args: Vec<String>,
    #[serde(default)]
    stdin: String,
}

/// Open the call envelope if `args` asks for one (arch §5.10.5). Not envelope mode →
/// `args` back untouched and `None` (the caller keeps its own stdin; `stdin` is not
/// read). Envelope mode → the inner `Args` (the envelope's `args`, the SAME
/// environment, stdin never a tty) and `Some(bytes)` to serve as the run's stdin. A
/// refusal is written to `stderr` and returned as its exit code (64).
pub fn unwrap_envelope(
    mut args: Args,
    stdin: &mut dyn Read,
    stderr: &mut dyn Write,
) -> Result<(Args, Option<Vec<u8>>), u8> {
    if args.argv.first().map(String::as_str) != Some(FLAG) {
        return Ok((args, None));
    }
    match open(&args.argv, stdin) {
        Ok(call) => {
            args.argv = call.args;
            args.tty = false;
            Ok((args, Some(call.stdin.into_bytes())))
        }
        Err(message) => {
            let _ = writeln!(stderr, "{message}");
            Err(ExitClass::Usage.code())
        }
    }
}

/// Read, parse and filter the envelope. No message echoes a VALUE the sender wrote —
/// only a flag's own name — so a refused credential is not copied into a log.
fn open(argv: &[String], stdin: &mut dyn Read) -> Result<Call, String> {
    if argv.len() > 1 {
        return Err(format!(
            "`{FLAG}` takes no other argument: the command line is the `args` of the JSON object on stdin"
        ));
    }
    let mut raw = Vec::new();
    stdin
        .read_to_end(&mut raw)
        .map_err(|e| format!("cannot read the envelope from stdin: {e}"))?;
    let call: Call = serde_json::from_slice(&raw).map_err(|e| {
        format!(
            "malformed envelope at line {} column {}: expected {{\"args\": [string…], \"stdin\": string}}",
            e.line(),
            e.column()
        )
    })?;
    let mut words = call.args.iter();
    while let Some(word) = words.next() {
        let key = word.split_once('=').map_or(word.as_str(), |(k, _)| k);
        if !ALLOWED.contains(&key) {
            let named = if key.starts_with('-') {
                format!("flag `{key}`")
            } else {
                "a positional prompt".to_owned()
            };
            return Err(format!(
                "{named} may not be sent in an envelope (allowed: {})",
                ALLOWED.join(", ")
            ));
        }
        // `--provider R` consumes its value, exactly as `parse_args` will; a missing
        // one is that parser's own 64.
        if word == "--provider" {
            words.next();
        }
    }
    Ok(call)
}
