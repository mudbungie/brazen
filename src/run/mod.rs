//! The `run` spine (arch §1, §4.4) — the whole binary behind one signature, pure
//! relative to its injected seams (`Transport`/`CredStore`/`Clock` + the three
//! writers). Two phases divided by the one boundary that matters: BEFORE the sink
//! exists, a failure is fatal and can only reach `stderr` (flag parse → 64,
//! input-open → 66, malformed config → 78); AFTER it, every failure is an in-band
//! `Event::Error` through the same sink, then the one `End`, then the exit (§5.9,
//! §8). No vendor name is ever matched — dispatch is the registry lookup (§4.4).
//! The response-driving half (frame → decode → project, with the exit-code
//! bookkeeping) lives in [`respond`].

mod respond;

use std::io::{Read, Write};

use crate::auth::AuthCtx;
use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind, Event, ExitClass};
use crate::cli::{parse_args, Args};
use crate::config::partial::OutMode;
use crate::config::{
    config_path, defaults, dump_config, fill_absent, partial_from_env, read_config_file,
    EnvSnapshot, PartialConfig,
};
use crate::pipeline::{open_input, read_request, NdjsonSink, RawSink, Sink, TextSink};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::registry::Registry;
use crate::store::{Clock, CredStore};
use crate::transport::Transport;

use respond::{drive, write_event};

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
    serve(
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

/// The post-sink pipeline (§4.4): read → resolve → dispatch → encode → auth →
/// send → drive. Every error is written in-band and ends the run with its exit
/// code; `merged` is consumed by resolution.
#[allow(clippy::too_many_arguments)]
fn serve(
    reader: &mut dyn Read,
    raw: bool,
    prompt: Option<String>,
    merged: PartialConfig,
    sink: &mut dyn Sink,
    transport: &dyn Transport,
    store: &dyn CredStore,
    clock: &dyn Clock,
) -> u8 {
    // Input: raw stdin bytes verbatim, or the canonical request (positional XOR
    // stdin). The mode was resolved before input, so this never branches on body.
    let input = if raw {
        match read_to_vec(reader) {
            Ok(bytes) => Input::Raw(bytes),
            Err(e) => return fail_inband(sink, e),
        }
    } else {
        match read_request(prompt.as_deref(), reader) {
            Ok(req) => Input::Canonical(req),
            Err(e) => return fail_inband(sink, e),
        }
    };

    // The request's model wins for routing when set; cloned so resolution does not
    // borrow `input` (which is moved into the wire below).
    let req_model = match &input {
        Input::Canonical(req) if !req.model.is_empty() => Some(req.model.clone()),
        _ => None,
    };
    let cfg = match merged.into_resolved(req_model.as_deref()) {
        Ok(c) => c,
        Err(e) => return fail_inband(sink, e.into()),
    };

    let registry = Registry::builtin();
    // Every `ProtocolId` AND `AuthId` is registered in `builtin` (a closed-enum
    // invariant), so both lookups are infallible — an `oauth2` row that cannot run
    // is already surfaced earlier, at resolve, as a missing `oauth` block (78).
    #[allow(clippy::expect_used)]
    let proto = registry
        .protocol(cfg.provider.protocol)
        .expect("every ProtocolId is registered in Registry::builtin");
    #[allow(clippy::expect_used)]
    let auth = registry
        .auth(cfg.provider.auth)
        .expect("every AuthId is registered in Registry::builtin");

    let beta: Vec<(&str, &str)> = cfg
        .provider
        .beta_headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let ctx = ProviderCtx {
        base_url: &cfg.provider.base_url,
        model: &cfg.model,
        api_header: &cfg.provider.api_header,
        beta_headers: &beta,
        extra: &cfg.extra,
    };
    let authc = AuthCtx {
        store_key: &cfg.provider.name,
        inline_key: cfg.inline_key.as_ref(),
        oauth: cfg.provider.oauth.as_ref(),
    };

    let mut wire = match input {
        Input::Raw(bytes) => WireRequest::raw(bytes),
        Input::Canonical(mut req) => {
            fill_absent(&mut req, &cfg);
            match proto.encode(&req, &ctx) {
                Ok(w) => w,
                Err(e) => return fail_inband(sink, e),
            }
        }
    };
    if let Err(e) = auth.apply(&mut wire, &ctx, &authc, store, clock, transport) {
        return fail_inband(sink, e);
    }
    let resp = match transport.send(wire) {
        Ok(r) => r,
        Err(e) => return fail_inband(sink, e),
    };
    drive(sink, raw, proto, resp)
}

/// Either input channel after resolution: provider-native bytes (`--raw`, sent
/// verbatim) or a canonical request (encoded). The mode picks the variant once.
enum Input {
    Raw(Vec<u8>),
    Canonical(CanonicalRequest),
}

/// Emit a pre-streaming `CanonicalError` in-band, then the one `End`, returning
/// the exit (§8). Under `--raw` the sink drops the error line; the exit still
/// carries it (§5.4).
fn fail_inband(sink: &mut dyn Sink, err: CanonicalError) -> u8 {
    let mut exit = err.exit_code();
    match write_event(sink, Event::Error(err), &mut exit)
        .and_then(|()| write_event(sink, Event::End, &mut exit))
    {
        Ok(()) => exit,
        Err(code) => code,
    }
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

/// Read a byte source to end into a `Vec` (the `--raw` request body), mapping an
/// IO failure to an in-band input error (64).
fn read_to_vec(reader: &mut dyn Read) -> Result<Vec<u8>, CanonicalError> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).map_err(|e| CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("failed to read stdin: {e}"),
        provider_detail: None,
    })?;
    Ok(buf)
}
