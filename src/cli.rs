//! Flag parsing (arch §5.5, §5.9): argv → `Flags`, the flag-layer `PartialConfig`
//! plus the non-config flags (`--input`, `--config`) and the control short-circuit
//! flags (`--login`, `--list-models`, `--dump-config`, `--help`, `--version`) and
//! the positional prompt. Control operations are flags, never `argv[0]` verbs, so a
//! bare leading word is ALWAYS a prompt (§5.10.1). Pure over a `&[String]`, so every
//! flag and every usage error (exit 64) is a table test with no process argv. The
//! `Args` bundle is the one impurity injection point — `main` snapshots real
//! argv+env+`isatty` into it; the lib reads none directly (arch §6.5).

use std::path::PathBuf;

use crate::canonical::{CanonicalError, Content, ErrorKind};
use crate::config::partial::OutMode;
use crate::config::{EnvSnapshot, PartialConfig};
use crate::store::Secret;

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
    /// `--browser`: select the loopback browser login flow (else the headless device
    /// flow). Meaningful only with `--login`; inert otherwise (§5.10.1).
    pub browser: bool,
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
    Run,
}

/// Read the routing decision from argv (§5.10.1). A parse error (an unknown flag, two
/// combined control ops) routes to [`Route::Run`], whose lib entry re-parses and
/// surfaces the same error as the authoritative 64 — so routing owns no error path.
pub fn route(argv: &[String]) -> Route {
    match parse_args(argv) {
        Ok(f) if f.login => Route::Login,
        Ok(f) if f.list_models => Route::ListModels,
        _ => Route::Run,
    }
}

/// A flag/usage failure → exit 64 (arch §8). `kind` is always `Usage`; the
/// message says what would fix it.
fn usage(message: impl Into<String>) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Usage,
        message: message.into(),
        provider_detail: None,
    }
}

/// Parse argv into [`Flags`] (arch §5.5, §13.7). Recognized flags set the flag-layer
/// config or a pre-resolve field; an unknown `--flag` or a missing value is a usage
/// error (64). Option parsing stops at the **first operand** (the first argument that
/// is neither an option nor an option-value): from there **through EOF** every token
/// is the prompt, operands joined by a single space — so a multi-word prompt needs no
/// quoting, and any `-`/`--`/word *after* the prompt starts is inert text, never an
/// option (options-before-prompt, POSIX Utility Syntax Guideline 9 — `bz --json "q"`
/// selects JSON, `bz "q" --json` sends the prompt `q --json`). `--` ends options
/// without being prompt text — its tail (if any) through EOF is the prompt, so a
/// leading-dash prompt is reachable (`bz -- --weird`); an empty tail (`bz --`) leaves
/// no positional (the stdin/bare path). A lone `-` is itself the first operand (no
/// second stdin name, §5.5); any other `-`-leading first token is an option, so a
/// leading-flag typo is still caught (unknown → 64). A present positional **wins and
/// stdin is not read** (`read_request`, §5.5) — no two-inputs error, no tty probe.
/// Both `--key value` and `--key=value` value forms are accepted.
pub fn parse_args(argv: &[String]) -> Result<Flags, CanonicalError> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        // `--` terminates options: its tail through EOF (if any) is the prompt, joined
        // by one space — never itself prompt text. An empty tail leaves no positional.
        if arg == "--" {
            let tail = &argv[i + 1..];
            if !tail.is_empty() {
                flags.prompt = Some(tail.join(" "));
            }
            break;
        }
        // The first operand (a non-option, or the lone `-`) stops option parsing: this
        // token through EOF is the prompt, the operand tail joined by a single space.
        if !arg.starts_with('-') || arg == "-" {
            flags.prompt = Some(argv[i..].join(" "));
            break;
        }
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        let cfg = &mut flags.config;
        match key {
            "--text" => cfg.output = Some(OutMode::Text),
            "--json" => cfg.output = Some(OutMode::Ndjson),
            "--raw" => cfg.output = Some(OutMode::Raw),
            "--thinking" => cfg.thinking = Some(true),
            "--stream" => cfg.stream = Some(true),
            // The non-stream tri-state intent (config §4.2): honored, never silently
            // reverted — `serve` folds a single-JSON 2xx body via `decode_full`. The
            // `--stream` sibling; `BRAZEN_STREAM=false` is the env form.
            "--no-stream" => cfg.stream = Some(false),
            "--dump-config" => flags.dump_config = true,
            // Control short-circuits (§5.10.1): each REPLACES the data-plane run with a
            // control action — never `argv[0]` verbs, so `bz "login"` stays a prompt.
            // `--browser` is meaningful only with `--login` (inert otherwise).
            "--login" => flags.login = true,
            "--list-models" => flags.list_models = true,
            "--browser" => flags.browser = true,
            // Discovery short-circuits (§5.5): each wins before resolution in `run`,
            // siblings of `--dump-config`. Set here so they stay pure table tests.
            "--help" | "-h" => flags.help = true,
            "--version" | "-V" => flags.version = true,
            "--provider" => cfg.provider = Some(value(key, inline, argv, &mut i)?),
            "--model" => cfg.model = Some(value(key, inline, argv, &mut i)?),
            "--api-key" => cfg.api_key = Some(Secret::new(value(key, inline, argv, &mut i)?)),
            "--max-tokens" => {
                cfg.max_tokens = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--temperature" => {
                cfg.temperature = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--top-p" => cfg.top_p = Some(number(key, value(key, inline, argv, &mut i)?)?),
            "--timeout-connect" => {
                cfg.timeout_connect = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--timeout-response" => {
                cfg.timeout_response = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--timeout-idle" => {
                cfg.timeout_idle = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            // The ergonomic single-string form of the leading system prompt: one
            // `Content::Text`, the same shape a bare file-array string decodes to.
            "--system" => cfg.system = Some(vec![Content::Text(value(key, inline, argv, &mut i)?)]),
            // `-f`/`--file` accumulates (content-attach, §5.5): each occurrence pushes
            // a path; the contents become one `Content::Text` part in `read_request`.
            "--file" | "-f" => flags
                .files
                .push(PathBuf::from(value(key, inline, argv, &mut i)?)),
            "--input" => flags.input = Some(PathBuf::from(value(key, inline, argv, &mut i)?)),
            "--config" => {
                flags.config_path = Some(PathBuf::from(value(key, inline, argv, &mut i)?))
            }
            _ => return Err(usage(format!("unknown flag `{key}` (try `bz --help`)"))),
        }
        i += 1;
    }
    // The three control operations are mutually exclusive; combining two is a usage
    // error (64, §5.10.1). The two PROBES (`--help`/`--version`) are exempt — a probe
    // answers first even alongside a control op (`bz --login --help` self-describes),
    // so the check is skipped when either is present.
    if !flags.help
        && !flags.version
        && u8::from(flags.dump_config) + u8::from(flags.list_models) + u8::from(flags.login) > 1
    {
        return Err(usage(
            "control operations --login / --list-models / --dump-config are mutually exclusive",
        ));
    }
    Ok(flags)
}

/// The value of a value-taking flag: the `--key=value` inline form if present,
/// else the following argv word (advancing the cursor onto it). A missing value
/// is a usage error (§5.9).
fn value(
    key: &str,
    inline: Option<String>,
    argv: &[String],
    i: &mut usize,
) -> Result<String, CanonicalError> {
    if let Some(v) = inline {
        return Ok(v);
    }
    *i += 1;
    argv.get(*i)
        .cloned()
        .ok_or_else(|| usage(format!("flag `{key}` needs a value")))
}

/// Parse a numeric flag value, mapping a parse failure to a usage error (64). A
/// semantic out-of-range (e.g. `max_tokens = 0`) is caught later by config
/// resolution (78), not here.
fn number<T: std::str::FromStr>(key: &str, raw: String) -> Result<T, CanonicalError> {
    raw.parse()
        .map_err(|_| usage(format!("flag `{key}` needs a number, got `{raw}`")))
}
