//! The argv parser (arch §5.5, §5.9): `parse_args` and its per-value helpers —
//! the verb half of the flag layer; the parsed shapes (`Args`/`Flags`/`Route`)
//! live in the parent module. Pure over a `&[String]`, so every flag and every
//! usage error (exit 64) is a table test with no process argv.

use std::path::PathBuf;

use crate::canonical::{CanonicalError, Content, ErrorKind, ReasoningEffort};
use crate::config::partial::OutMode;
use crate::ingress::{dialect_id, IngressId};
use crate::store::Secret;

use super::Flags;

/// A flag/usage failure → exit 64 (arch §8). `kind` is always `Usage`; the
/// message says what would fix it.
fn usage(message: impl Into<String>) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Usage,
        message: message.into(),
        provider_detail: None,
        retry_after_seconds: None,
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
            // `--raw` is DIRECTIONAL (§5.4, §5.10.2): bare/`=both` is symmetric (both
            // halves raw), `=in` is verbatim-request-only, `=out` is verbatim-response-
            // only. The OUTPUT axis is `output = Raw` (the `RawSink`), set for every
            // spelling that streams raw bytes out (bare/`=both`/`=out`); the INPUT axis
            // is `raw_in`, set explicitly by `=in`/`=out` and left `None` for bare so it
            // DERIVES from the final `output` at resolve — that derivation is what keeps
            // `--raw --json` backward-compatible (a later `--json` moves `output` off Raw,
            // so bare-raw's input-rawness lapses), while an explicit `--raw=in` survives
            // it (§5.10.2). An unknown value (`--raw=foo`) is a usage error (64).
            "--raw" => raw_direction(inline.as_deref(), cfg)?,
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
            "--count-tokens" => flags.count_tokens = true,
            // The masquerade listener (ingress §7): a control-plane MODE flag of this
            // same family — it replaces the one-shot data plane with the accept loop.
            "--serve" => flags.serve = true,
            "--browser" => flags.browser = true,
            // The one-shot ingress filter (ingress §11): stdin carries ONE request in
            // the named client dialect. Explicit, never sniffed (§2) — an unknown
            // dialect name is a usage error (64), the flag-layer twin of the
            // `[ingress].dialect` config check (78).
            "--in" => flags.in_dialect = Some(dialect(key, value(key, inline, argv, &mut i)?)?),
            // Discovery short-circuits (§5.5): each wins before resolution in `run`,
            // siblings of `--dump-config`. Set here so they stay pure table tests.
            // `--skill` prints the fuller embedded skill doc; a probe like `--help`.
            "--skill" => flags.skill = true,
            "--help" | "-h" => flags.help = true,
            "--version" | "-V" => flags.version = true,
            "--provider" => cfg.provider = Some(value(key, inline, argv, &mut i)?),
            "--model" | "-m" => cfg.model = Some(value(key, inline, argv, &mut i)?),
            // The host override (config §4.5): replaces the RESOLVED row's base_url —
            // same provider, different endpoint (proxy/mock/vLLM/gateway) — so a harness
            // needs no temp config file. NOT a row injector; protocol/auth stay the row's.
            "--base-url" => cfg.base_url = Some(value(key, inline, argv, &mut i)?),
            "--api-key" => cfg.api_key = Some(Secret::new(value(key, inline, argv, &mut i)?)),
            "--max-tokens" => {
                cfg.max_tokens = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--temperature" => {
                cfg.temperature = Some(number(key, value(key, inline, argv, &mut i)?)?)
            }
            "--top-p" => cfg.top_p = Some(number(key, value(key, inline, argv, &mut i)?)?),
            // The portable reasoning knob (§5.3): a SEPARATE request flag from the
            // display-only `--thinking`; maps per-protocol at encode (providers.md §6).
            "--reasoning" => {
                cfg.reasoning = Some(reasoning(key, value(key, inline, argv, &mut i)?)?)
            }
            // The transport SILENCE budget (§5.10.3, §13.15): ONE value fanned onto
            // ureq's connect / response-header / inter-chunk-idle budgets at resolve.
            // The three old `--timeout-*` flags collapsed here; a stray one is now an
            // unknown flag (64), the general no-per-flag-knowledge path.
            "--timeout" => cfg.timeout = Some(number(key, value(key, inline, argv, &mut i)?)?),
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
    // The control operations are mutually exclusive; combining two is a usage error
    // (64, §5.10.1). The PROBES (`--help`/`--version`/`--skill`) are exempt — a probe
    // answers first even alongside a control op (`bz --login --help` self-describes), so
    // the check is skipped when any is present.
    if !flags.help
        && !flags.version
        && !flags.skill
        && u8::from(flags.dump_config)
            + u8::from(flags.list_models)
            + u8::from(flags.login)
            + u8::from(flags.count_tokens)
            + u8::from(flags.serve)
            > 1
    {
        return Err(usage(
            "control operations --login / --list-models / --count-tokens / --dump-config / --serve are mutually exclusive",
        ));
    }
    // `--in` reads ONE dialect request from stdin (ingress §11) — a positional
    // prompt names the OTHER input contract, so the two cannot combine (64). The
    // `--raw=in` conflict is checked in `run`, where the derived input axis exists.
    if flags.in_dialect.is_some() && flags.prompt.is_some() {
        return Err(usage(
            "--in reads one dialect request from stdin and cannot be combined with a positional prompt",
        ));
    }
    Ok(flags)
}

/// Parse the `--in` value onto the closed dialect set (ingress §2, §11): named
/// explicitly, never sniffed; an unknown name is a usage error (64).
fn dialect(key: &str, raw: String) -> Result<IngressId, CanonicalError> {
    dialect_id(&raw).ok_or_else(|| {
        usage(format!(
            "flag `{key}` needs a known ingress dialect (openai_chat, anthropic_messages), got `{raw}`"
        ))
    })
}

/// Apply a `--raw[=DIR]` spelling to the two rawness axes (§5.4, §5.10.2). Bare
/// `--raw` and `--raw=both` are symmetric: only `output = Raw` (the OUTPUT axis), the
/// INPUT axis left to DERIVE from the final `output`. `--raw=in` sets ONLY the input
/// axis (`raw_in = true`) — no `output` change, so it composes with `--text`/`--json`.
/// `--raw=out` sets `output = Raw` and pins the input axis normal (`raw_in = false`).
/// Any other value is a usage error (64). Both axes are last-wins across repeats.
fn raw_direction(
    dir: Option<&str>,
    cfg: &mut crate::config::PartialConfig,
) -> Result<(), CanonicalError> {
    match dir {
        None | Some("both") => cfg.output = Some(OutMode::Raw),
        Some("in") => cfg.raw_in = Some(true),
        Some("out") => {
            cfg.output = Some(OutMode::Raw);
            cfg.raw_in = Some(false);
        }
        Some(other) => {
            return Err(usage(format!(
                "flag `--raw` takes no value or `in`/`out`/`both`, got `{other}`"
            )))
        }
    }
    Ok(())
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

/// Parse the `--reasoning` value (`low|medium|high`), mapping anything else to a
/// usage error (64) — the flag-layer twin of `BRAZEN_REASONING`'s `BadValue`.
fn reasoning(key: &str, raw: String) -> Result<ReasoningEffort, CanonicalError> {
    raw.parse()
        .map_err(|()| usage(format!("flag `{key}` needs low|medium|high, got `{raw}`")))
}
