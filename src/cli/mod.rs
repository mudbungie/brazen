//! The flag layer (arch §5.5, §5.9): argv → `Flags`, the flag-layer `PartialConfig`
//! plus the non-config flags (`--input`, `--config`) and the control short-circuit
//! flags (`--login`, `--list-models`, `--list-providers`, `--dump-config`, `--help`,
//! `--version`) and
//! the positional prompt. Control operations are flags, never `argv[0]` verbs, so a
//! bare leading word is ALWAYS a prompt (§5.10.1). This module holds the parsed
//! SHAPES (`Args`/`Flags`/`Route`); the parser itself lives in [`parse`]. The
//! `Args` bundle is the one impurity injection point — `main` snapshots real
//! argv+env+`isatty` into it; the lib reads none directly (arch §6.5).

use std::path::PathBuf;

use crate::config::{EnvSnapshot, PartialConfig};
use crate::ingress::IngressId;

mod parse;

pub use parse::parse_args;

/// The injected process inputs handed to [`run`](crate::run()): the program
/// arguments (excluding `argv[0]`), a snapshot of the environment, and the one bit
/// of terminal state the pure lib can't observe — whether stdin is an interactive
/// tty (§5.5). `main` builds it from `std::env`/`isatty`; tests build it from
/// literals — so `run` is exercised end-to-end without touching the real process
/// state (arch §6.5, §9.6).
pub struct Args {
    pub argv: Vec<String>,
    pub env: EnvSnapshot,
    /// Is stdin an interactive terminal? The shim probes `isatty(0)` (the impurity
    /// kept out of the pure lib, §5.5) and injects the fact here; `run` reads it to
    /// turn a bare interactive invocation (tty, no prompt, no stdin request) into
    /// the friendly usage hint instead of an empty-stdin parse error. A pipe is
    /// `false`, so the piped/scripted path is unchanged.
    pub tty: bool,
    /// Is **stdout** an interactive terminal? The second isatty fact, probed the same
    /// way (`isatty(1)`, the sibling of the stdin `tty` above, interactive-output spec
    /// §2) and injected here; `run` feeds it to `Style::resolve` to pick the pretty
    /// text skin. A pipe/redirect/non-unix is `false`, so the building-block stdout
    /// contract is unchanged — pretty never activates off a tty.
    pub stdout_tty: bool,
}

/// The parsed flag layer (arch §5.5). `config` is the flag-encoded
/// `PartialConfig` (highest-precedence fold operand); the rest are pre-resolve
/// concerns: `input`/`config_path` name *which file*, the control flags
/// (`login`/`list_models`/`dump_config`/`help`/`version`) select a short-circuit,
/// and `prompt` is the positional argv request channel.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct Flags {
    pub config: PartialConfig,
    pub prompt: Option<String>,
    /// `-f`/`--file <path>`, REPEATABLE — accumulates (NOT last-wins, unlike
    /// `--input`). Each path's contents become one `Content::Text` part prepended,
    /// in argv order, before the positional prompt in the one user message
    /// (content-attach, §5.5). Empty = no attachments (the general path).
    pub files: Vec<PathBuf>,
    pub input: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub dump_config: bool,
    /// `--login`: obtain+store an OAuth/SSO credential for the resolved `--provider`
    /// — a control short-circuit (§5.10.1), never an `argv[0]` verb. The shim wires
    /// the interactive seams when this is set ([`route`]).
    pub login: bool,
    /// `--list-models`: one GET listing the resolved provider's models — the sibling
    /// control short-circuit (model-discovery §2), the cache's wholesale writer (the
    /// data plane appends learned ids on success, §5.4).
    pub list_models: bool,
    /// `--list-providers`: the EFFECTIVE provider table (config §6.1) — the LOCAL
    /// sibling of `--list-models`, which lists one row's models. Zero round-trips: it
    /// folds the config the way a run does (defaults operand INCLUDED, unlike
    /// `--dump-config`) and completes every row. Routes to none.
    pub list_providers: bool,
    /// `--count-tokens`: one round-trip to the provider's count endpoint, returning a
    /// provider-accurate `input_tokens` for the request read the SAME way the data plane
    /// reads one (§5.10.1). A control short-circuit; no cache write. A provider with no
    /// count endpoint declines (Config, 78).
    pub count_tokens: bool,
    /// `--serve`: enter the masquerade accept loop (ingress §7) — a control-plane
    /// MODE flag of the same short-circuit family, replacing the one-shot data
    /// plane. The shim wires the listener seams when this is set ([`route`]);
    /// `--serve` without an `[ingress]` table refuses (Config, 78).
    pub serve: bool,
    /// `--in DIALECT`: the one-shot ingress filter (ingress §11) — stdin carries
    /// ONE request in this client dialect; stdout carries the dialect response
    /// (aggregate, or SSE when the request says `stream:true`). A DATA-plane
    /// input contract, not a control op: `run` dispatches on it. Mutually
    /// exclusive with a positional prompt and with `--raw=in` (64).
    pub in_dialect: Option<IngressId>,
    /// `--browser`: select the loopback browser login flow (else the headless device
    /// flow). Meaningful only with `--login`; inert otherwise (§5.10.1).
    pub browser: bool,
    /// `--skill`: print the embedded agent-facing skill doc (`SKILL.md`, richer
    /// than `--help` with worked examples) to stdout, exit 0. A discovery probe of the
    /// same short-circuit family as `--help`/`--version` — answers before resolution and
    /// alongside a control op — so it, too, is exempt from the mutual-exclusion check.
    pub skill: bool,
    /// `--help`: print the one-screen usage to stdout, exit 0. A discovery probe,
    /// so it short-circuits before resolution — a sibling of `dump_config`.
    pub help: bool,
    /// `--version`: print the package version to stdout, exit 0. Same short-circuit.
    pub version: bool,
}

/// Which control plane the `bz` shim should wire (§5.10.1). Computed by the ONE
/// authoritative [`parse_args`], so the coverage-excluded shim never hand-rolls an
/// argv scan and can never disagree with the lib on flag-vs-prompt: a value whose
/// text looks like a control flag (`--system=--login`) is the value, and any word
/// after `--` is the prompt, so neither is ever mistaken for a route.
pub enum Route {
    Login,
    ListModels,
    /// `--list-providers` (config §6.1): the shim wires stdout/stderr + the credential
    /// store and NOTHING else — no transport, no clock, no cache — so the listing is
    /// offline by construction.
    ListProviders,
    CountTokens,
    /// `--serve` (ingress §7): the shim wires the TCP `Bind` seam + the replay
    /// stash and enters the accept loop instead of the one-shot data plane.
    Serve,
    Run,
}

/// Read the routing decision from argv (§5.10.1). A parse error (an unknown flag, two
/// combined control ops) routes to [`Route::Run`], whose lib entry re-parses and
/// surfaces the same error as the authoritative 64 — so routing owns no error path.
pub fn route(argv: &[String]) -> Route {
    match parse_args(argv) {
        Ok(f) if f.login => Route::Login,
        Ok(f) if f.list_models => Route::ListModels,
        Ok(f) if f.list_providers => Route::ListProviders,
        Ok(f) if f.count_tokens => Route::CountTokens,
        Ok(f) if f.serve => Route::Serve,
        _ => Route::Run,
    }
}
