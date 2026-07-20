//! `bz --serve` (ingress.md §7, §8): the control-plane accept loop — a shell
//! around the unchanged pipeline. Thread-per-connection over the injected
//! [`Bind`]/[`Listener`] seams, `std::thread` scoped and blocking end to end:
//! N concurrent connections are N independent pipelines, exactly as N `bz`
//! processes would be (the data plane itself never fans out). Each connection
//! serves keep-alive requests serially; the pseudo-routes (§8) answer the
//! non-generation calls real harnesses make. The loop runs until the listener
//! yields `None` — natively that is never (SIGINT/SIGTERM end the process, the
//! repo's default-disposition convention); in tests the scripted queue drains.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::thread;

use serde_json::{json, Value};

use crate::canonical::{CanonicalError, ErrorKind};
use crate::cli::{parse_args, Args};
use crate::config::errors::ConfigError;
use crate::config::partial::LossyMode;
use crate::config::{
    config_path, defaults, partial_from_env, read_config_file, IngressConfig, PartialConfig,
};
use crate::ingress::{dialect_id, IngressId, THINKING_REPLAY};
use crate::store::{Clock, CredStore, ModelCache, ReplayStash, Secret};
use crate::transport::Transport;

use super::http::{read_request, HttpRequest, HttpRespond};
use super::{edge, turn, Bind, Host, MasqIn, Respond, ServeConn};

/// The injected seams + writers for one `bz --serve` (ingress §7) — the sibling
/// of `ListIo`/`LoginIo`. The data-plane seams carry `Sync` here because the
/// connection threads share them (each thread rebuilds the ordinary [`Host`]
/// view); `bind` yields the listener; `stash` is the §5 replay stash.
pub struct ServeIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub bind: &'a dyn Bind,
    pub transport: &'a (dyn Transport + Sync),
    pub store: &'a (dyn CredStore + Sync),
    pub cache: &'a (dyn ModelCache + Sync),
    pub clock: &'a (dyn Clock + Sync),
    pub stash: &'a ReplayStash,
}

/// Run `bz --serve` and return the POSIX exit code (ingress §7). Pre-loop
/// failures are fatal and stderr-only, exactly like every control op: flag
/// parse 64, config (no/invalid `[ingress]` table, unknown dialect) 78, a bind
/// failure 69. Once the loop runs, every failure is a per-connection concern
/// answered in the client dialect (§9) — the process stays up.
pub fn serve(args: &Args, io: &mut ServeIo) -> u8 {
    match run_serve(args, io) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_serve(args: &Args, io: &mut ServeIo) -> Result<u8, CanonicalError> {
    let mut flags = parse_args(&args.argv)?;
    // The discovery probes win first, as on every entry (§5.5).
    if flags.help {
        return Ok(super::super::emit(io.stdout, super::super::HELP));
    }
    if flags.skill {
        return Ok(super::super::emit(io.stdout, super::super::SKILL));
    }
    if flags.version {
        return Ok(super::super::emit(io.stdout, super::super::VERSION_LINE));
    }
    let file = read_config_file(&config_path(flags.config_path.take(), &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    // The `[ingress]` table, resolved and validated (§6): dialect required,
    // listen parsed + the non-loopback-without-token refusal, overrides checked.
    let ing: IngressConfig = merged.resolve_ingress().map_err(CanonicalError::from)?;
    let dialect = dialect_id(&ing.dialect).ok_or_else(|| {
        CanonicalError::from(ConfigError::Ingress {
            detail: format!(
                "unknown ingress dialect `{}` (known: openai_chat, anthropic_messages)",
                ing.dialect
            ),
        })
    })?;
    let listener = io.bind.bind(ing.listen).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("cannot bind {}: {e}", ing.listen),
        provider_detail: None,
        retry_after_seconds: None,
    })?;
    let cx = ServeCx {
        dialect,
        reject: ing.lossy_for(THINKING_REPLAY) == LossyMode::Reject,
        token: ing.token.clone(),
        merged: &merged,
        transport: io.transport,
        store: io.store,
        cache: io.cache,
        clock: io.clock,
        stash: io.stash,
    };
    // Thread-per-connection (§7): scoped, so the shared context is borrowed,
    // not cloned per thread; the scope joins every connection before returning.
    thread::scope(|s| {
        while let Some(conn) = listener.accept() {
            let cx = &cx;
            s.spawn(move || connection(conn, cx));
        }
    });
    Ok(0)
}

/// Everything a connection thread shares, read-only (`Sync` by construction).
struct ServeCx<'a> {
    dialect: IngressId,
    reject: bool,
    token: Option<Secret>,
    merged: &'a PartialConfig,
    transport: &'a (dyn Transport + Sync),
    store: &'a (dyn CredStore + Sync),
    cache: &'a (dyn ModelCache + Sync),
    clock: &'a (dyn Clock + Sync),
    stash: &'a ReplayStash,
}

/// One connection's keep-alive loop (§7): requests served serially by this
/// thread until the client hangs up, asks `Connection: close`, breaks the HTTP
/// grammar (400, then close — the framing is unrecoverable), or a write fails
/// (a mid-stream disconnect: the turn already dropped its upstream; only this
/// connection dies).
fn connection(conn: Box<dyn ServeConn>, cx: &ServeCx) {
    let host = Host {
        transport: cx.transport,
        store: cx.store,
        cache: cx.cache,
        clock: cx.clock,
        stash: cx.stash,
    };
    let mut reader = BufReader::new(conn);
    loop {
        let req = match read_request(&mut reader) {
            Ok(Some(req)) => req,
            Ok(None) => return,
            Err(detail) => {
                let mut out = HttpRespond::new(reader.get_mut());
                let _ = edge(cx.dialect, malformed(detail), cx.clock, &mut out);
                return;
            }
        };
        let close = req.wants_close();
        let mut out = HttpRespond::new(reader.get_mut());
        respond(&req, cx, &host, &mut out);
        if out.dead() || close {
            return;
        }
    }
}

/// Auth, then the pseudo-routes (§7, §8): the bearer gate covers every route;
/// `POST /v1/chat/completions` is the data route, `GET /v1/models` the local
/// re-encode of the model cache, anything else the dialect's 404 envelope.
fn respond(req: &HttpRequest, cx: &ServeCx, host: &Host, out: &mut HttpRespond) {
    if let Some(token) = &cx.token {
        let want = format!("Bearer {}", token.expose());
        if req.header("authorization") != Some(want.as_str()) {
            let _ = edge(cx.dialect, unauthorized(), cx.clock, out);
            return;
        }
    }
    let path = req.path.split('?').next().unwrap_or_default();
    match (req.method.as_str(), path) {
        ("POST", "/v1/chat/completions") => {
            let masq = MasqIn {
                dialect: cx.dialect,
                reject: cx.reject,
                merged: cx.merged.clone(),
                stash: cx.stash,
            };
            // The exit code is the filter's concern; here the §9 status carried it.
            let _ = turn(masq, &req.body, host, out);
        }
        ("GET", "/v1/models") => {
            let body = models_body(cx.merged, host.cache, cx.clock.now());
            let _ = out
                .begin(200, false)
                .and_then(|()| out.chunk(&body))
                .and_then(|()| out.end());
        }
        (method, path) => {
            let _ = edge(cx.dialect, not_found(method, path), cx.clock, out);
        }
    }
}

/// `GET /v1/models` (§8): the existing per-provider model cache re-encoded as
/// the dialect's model list, UNION every row's `model_aliases` keys — precisely
/// the names a masquerade client is expected to ask for. Cold cache → aliases
/// only; brazen NEVER lists upstream automatically — refreshing is the
/// operator's `bz --list-models`. `owned_by` is the routing row's name.
fn models_body(merged: &PartialConfig, cache: &dyn ModelCache, created: u64) -> Vec<u8> {
    let mut ids: BTreeMap<String, &str> = BTreeMap::new();
    for (name, row) in &merged.providers {
        for model in cache.get(name).unwrap_or_default() {
            ids.insert(model.id, name);
        }
        for alias in row.model_aliases.iter().flat_map(BTreeMap::keys) {
            ids.insert(alias.clone(), name);
        }
    }
    let data: Vec<Value> = ids
        .into_iter()
        .map(|(id, owner)| {
            json!({"created": created, "id": id, "object": "model", "owned_by": owner})
        })
        .collect();
    json!({"data": data, "object": "list"})
        .to_string()
        .into_bytes()
}

/// Malformed HTTP → the dialect 400 (§7, §9): `ParseInput`, brazen as origin.
fn malformed(detail: String) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("malformed HTTP request: {detail}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// The §7 bearer gate's 401, in the dialect envelope. Client-supplied API keys
/// are otherwise ignored — upstream auth is brazen's own, as in one-shot mode.
fn unauthorized() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Auth,
        message: "missing or invalid bearer token".into(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// Any route but the two pseudo-routes → the dialect 404 envelope (§8).
fn not_found(method: &str, path: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Provider { status: 404 },
        message: format!("no route for {method} {path}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
