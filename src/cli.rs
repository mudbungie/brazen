//! Flag parsing (arch §5.5, §5.9): argv → `Flags`, the flag-layer `PartialConfig`
//! plus the three non-config flags (`--input`, `--config`, `--dump-config`) and
//! the positional prompt. Pure over a `&[String]`, so every flag and every usage
//! error (exit 64) is a table test with no process argv. The `Args` bundle is the
//! one impurity injection point — `main` snapshots real argv+env into it; the lib
//! reads neither directly (arch §6.5).

use std::path::PathBuf;

use crate::canonical::{CanonicalError, Content, ErrorKind};
use crate::config::partial::OutMode;
use crate::config::{EnvSnapshot, PartialConfig};
use crate::store::Secret;

/// The injected process inputs handed to [`run`](crate::run): the program
/// arguments (excluding argv[0]) and a snapshot of the environment. `main` builds
/// it from `std::env`; tests build it from literals — so `run` is exercised
/// end-to-end without touching the real process state (arch §6.5, §9.6).
pub struct Args {
    pub argv: Vec<String>,
    pub env: EnvSnapshot,
}

/// The parsed flag layer (arch §5.5). `config` is the flag-encoded
/// `PartialConfig` (highest-precedence fold operand); the rest are pre-resolve
/// concerns: `input`/`config_path` name *which file*, `dump_config` selects the
/// config-dump control path, and `prompt` is the positional argv request channel.
#[derive(Debug, Default)]
pub struct Flags {
    pub config: PartialConfig,
    pub prompt: Option<String>,
    pub input: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub dump_config: bool,
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

/// Parse argv into [`Flags`] (arch §5.5). Recognized flags set the flag-layer
/// config or a pre-resolve field; an unknown `--flag` or a missing value is a
/// usage error (64); a non-flag arg is the one positional prompt (a second is an
/// error). `--` ends option parsing (getopt) so a `-`-leading prompt is reachable
/// after it; a lone `-` is itself a positional, never a flag (no second stdin
/// name, §5.5). Both `--key value` and `--key=value` value forms are accepted.
pub fn parse_args(argv: &[String]) -> Result<Flags, CanonicalError> {
    let mut flags = Flags::default();
    let mut i = 0;
    let mut opts_ended = false;
    while i < argv.len() {
        let arg = &argv[i];
        if opts_ended || !arg.starts_with('-') || arg == "-" {
            set_prompt(&mut flags, arg)?;
            i += 1;
            continue;
        }
        if arg == "--" {
            opts_ended = true;
            i += 1;
            continue;
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
            "--input" => flags.input = Some(PathBuf::from(value(key, inline, argv, &mut i)?)),
            "--config" => {
                flags.config_path = Some(PathBuf::from(value(key, inline, argv, &mut i)?))
            }
            _ => return Err(usage(format!("unknown flag `{key}`"))),
        }
        i += 1;
    }
    Ok(flags)
}

/// Record the positional prompt; a second positional is a usage error — never a
/// silent join or pick (§5.5).
fn set_prompt(flags: &mut Flags, arg: &str) -> Result<(), CanonicalError> {
    if flags.prompt.is_some() {
        return Err(usage("only one positional prompt is allowed"));
    }
    flags.prompt = Some(arg.to_owned());
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
