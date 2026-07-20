//! Model discovery (model-discovery §2, §5): the `bz --list-models` control flag — the
//! WHOLESALE writer of the model cache (it REPLACES the list) and the ONLY model-list
//! fetch in `bz`. The generation path reads the cache this verb wrote and NEVER GETs
//! `/models`; its own cache write is the narrow learn-on-success append of one id
//! (model-discovery §5.4, in `generate`), never a list. This module is the VERB —
//! flag parse, resolve, print, the cache write; the wire half (the effective request
//! shape + the one GET/drain/decode) lives in [`fetch`].

use std::io::Write;

use crate::canonical::{CanonicalError, ErrorKind, Model};
use crate::config::{
    config_path, defaults, partial_from_env, read_config_file, OutMode, ResolvedConfig,
};
use crate::store::{Clock, CredStore, ModelCache};
use crate::transport::Transport;

mod fetch;

use fetch::fetch_models;
#[cfg(test)]
pub(crate) use fetch::models_req;

/// The injected seams + writers for one `bz --list-models` (model-discovery §2), the
/// sibling of `LoginIo`. The verb writes its listing to `stdout` and any error to
/// `stderr`, reuses the data-plane `Transport`/`CredStore`/`Clock` for the one GET
/// (auth/refresh and all, through the same `Auth::apply` seam), and is the WHOLESALE
/// writer of the `cache` — it `put`s the decoded list the generation path later reads,
/// which that path then appends learned ids to on success (§5, §5.4).
pub struct ListIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub transport: &'a dyn Transport,
    pub store: &'a dyn CredStore,
    pub cache: &'a dyn ModelCache,
    pub clock: &'a dyn Clock,
}

/// Run `bz --list-models` and return the POSIX exit code (model-discovery §2). Reuses
/// the full flag parser + `into_resolved(None, …)` to pick the provider (an explicit
/// `--provider`, else the row owning a configured `model`; neither → `NoProvider`/78),
/// does ONE GET to `models_path`, and prints — `--json` the `{"models":[…]}` object,
/// else the ids one per line with ` (default)` on the default. The listing goes to
/// stdout; any failure is written to stderr and mapped to its exit (config 78 / auth
/// 77 / non-2xx 69-70 / a malformed body 70 — the same run-level table).
pub fn list_models(args: &crate::cli::Args, io: &mut ListIo) -> u8 {
    match run_list(args, io) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_list(args: &crate::cli::Args, io: &mut ListIo) -> Result<u8, CanonicalError> {
    let flags = crate::cli::parse_args(&args.argv)?;
    // The discovery short-circuits ride the SAME flag layer and the SAME doc as the
    // data plane (§5.5): `bz --list-models --help`/`--version` self-describe to stdout
    // and exit 0 BEFORE any config/network — a probe must answer with no provider.
    if flags.help {
        return Ok(super::emit(io.stdout, super::HELP));
    }
    if flags.skill {
        return Ok(super::emit(io.stdout, super::SKILL));
    }
    if flags.version {
        return Ok(super::emit(io.stdout, super::VERSION_LINE));
    }
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    let cfg: ResolvedConfig = merged
        .into_resolved(None, Some(io.cache))
        .map_err(CanonicalError::from)?;
    // The verb's output shape is the SAME resolved fact the data plane folds (run::run),
    // not the flag layer alone: `--json`, `BRAZEN_OUTPUT=ndjson`, and a config-file
    // `output = "ndjson"` all select the object form, exactly as they do for generation.
    let json = cfg.output == OutMode::Ndjson;
    let models = fetch_models(&cfg, io.transport, io.store, io.clock)?;
    // Write the cache — the WHOLESALE write site (model-discovery §5): this REPLACES the
    // list (carrying `last_used` forward — re-listing changes which ids EXIST, never which
    // one you last USED), whereas the generation path appends one learned id and moves the
    // pointer (§5.4). Best-effort:
    // `put` is atomic + warns on its own IO failure (the impl's concern), so the verb's
    // exit is exactly the listing's, never the cache write's. The generation path reads this.
    let prior = io.cache.get(&cfg.provider.name).unwrap_or_default();
    io.cache
        .put(&cfg.provider.name, &prior.relist(models.clone()));
    print_models(io.stdout, &models, json).map_err(write_failed)?;
    if models.is_empty() {
        // A well-formed EMPTY 2xx is a successful empty listing — exit 0, the verb
        // LISTS, it does not select (model-discovery §2). Surface it on stderr so an
        // empty list is never a silent void: a `[provider.models].query` pin can be
        // server-side version-gated (§3.2) and silently return empty, so this is the
        // documented, observable behavior of a stale pin — NOT a changed exit.
        let _ = writeln!(io.stderr, "no models returned for `{}`", cfg.provider.name);
    }
    Ok(0)
}

/// Print the model list (model-discovery §2): `--json` the one `{"models":[…]}`
/// object (serde-direct, like the event stream), else the ids one per line in
/// provider order, the default-flagged one suffixed ` (default)`.
fn print_models(out: &mut dyn Write, models: &[Model], json: bool) -> std::io::Result<()> {
    if json {
        let obj = serde_json::json!({ "models": models });
        writeln!(out, "{obj}")
    } else {
        for m in models {
            let suffix = if m.default { " (default)" } else { "" };
            writeln!(out, "{}{suffix}", m.id)?;
        }
        Ok(())
    }
}

/// A stdout write failure for the listing → `Transport` (→69), the verb's pre-sink
/// analogue of the data plane's `BrokenPipe`/write handling.
fn write_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to write model list: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

#[cfg(test)]
mod tests {
    use super::print_models;
    use crate::canonical::Model;

    /// The ` (default)` suffix in text mode is unreachable from a real listing — no
    /// dialect flags a default today, so every decoded `Model` is `default:false`
    /// (model-discovery §3.1). The seam stays so a provider that DOES flag one needs no
    /// code change; this exercises that branch directly with a hand-built list (the
    /// `os::browser` precedent for a branch the integration surface cannot reach).
    #[test]
    fn text_suffixes_the_default_flagged_id() {
        let models = [
            Model {
                id: "fast".into(),
                default: false,
                ..Default::default()
            },
            Model {
                id: "smart".into(),
                default: true,
                ..Default::default()
            },
        ];
        let mut out = Vec::new();
        print_models(&mut out, &models, false).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "fast\nsmart (default)\n");
    }
}
