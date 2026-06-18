//! Model discovery (model-discovery §2, §5.2): the ONE models-list round-trip that
//! both the imprecise-model probe (serve) and the `bz list-models` verb run, plus
//! the verb itself. [`fetch_models`] is the single home for "GET `{base_url}`
//! `{models_path}`, auth, drain the 2xx body, decode" — the probe expands its seed
//! against the result via [`select_model`](crate::canonical::select_model); the verb
//! prints it. The GET carries the row's `beta_headers` (e.g. Anthropic's required
//! `anthropic-version`) exactly as `encode` does, since it skips `encode`.

use std::io::Write;

use crate::auth::AuthCtx;
use crate::canonical::{CanonicalError, ErrorKind, Model};
use crate::config::{
    config_path, defaults, partial_from_env, read_config_file, OutMode, ResolvedConfig,
};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::registry::Registry;
use crate::store::{Clock, CredStore};
use crate::transport::Transport;

/// The injected seams + writers for one `bz list-models` (model-discovery §2), the
/// sibling of `LoginIo`. The verb writes its listing to `stdout` and any error to
/// `stderr`, and reuses the data-plane `Transport`/`CredStore`/`Clock` for the one
/// GET (auth/refresh and all, through the same `Auth::apply` seam).
pub struct ListIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub transport: &'a dyn Transport,
    pub store: &'a dyn CredStore,
    pub clock: &'a dyn Clock,
}

/// Run `bz list-models` and return the POSIX exit code (model-discovery §2). Reuses
/// the full flag parser + `into_resolved(None)` to pick the provider (an explicit
/// `--provider`, else the row owning a configured `model`; neither → `NoProvider`/78),
/// does ONE GET to `models_path`, and prints — `--json` the `{"models":[…]}` object,
/// else the ids one per line with ` (default)` on the default. The listing goes to
/// stdout; any failure is written to stderr and mapped to its exit (config 78 / auth
/// 77 / non-2xx 69-70 / a malformed body 70 — the same run-level table).
pub fn list_models(args: &crate::cli::Args, io: &mut ListIo) -> u8 {
    match run_list(args, io) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_list(args: &crate::cli::Args, io: &mut ListIo) -> Result<(), CanonicalError> {
    let flags = crate::cli::parse_args(&args.argv[1..])?;
    let json = flags.config.output == Some(OutMode::Ndjson);
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    let cfg: ResolvedConfig = merged.into_resolved(None).map_err(CanonicalError::from)?;
    let models = fetch_models(&cfg, io.transport, io.store, io.clock)?;
    print_models(io.stdout, &models, json).map_err(write_failed)
}

/// The ONE models-list round-trip (model-discovery §5.2), shared by the probe and the
/// verb: GET `{base_url}{models_path}`, stamp the resolved timeouts, `Auth::apply`
/// (the same seam — api-key/bearer/oauth, refresh and all), send, drain the WHOLE 2xx
/// body, and `decode_models`. A non-2xx maps through `from_http_status` carrying the
/// status (4xx→69/auth-77, 5xx→70); a malformed 2xx body is the `Provider{502}`
/// `decode_models` raises. The GET carries the row's `beta_headers` because it skips
/// `encode`, which is where the generation path otherwise stamps them.
pub(crate) fn fetch_models(
    cfg: &ResolvedConfig,
    transport: &dyn Transport,
    store: &dyn CredStore,
    clock: &dyn Clock,
) -> Result<Vec<Model>, CanonicalError> {
    let registry = Registry::builtin();
    let proto = registry.protocol(cfg.provider.protocol);
    let auth = registry.auth(cfg.provider.auth);
    let beta: Vec<(&str, &str)> = cfg
        .provider
        .beta_headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let ctx = ProviderCtx {
        base_url: &cfg.provider.base_url,
        model: &cfg.model,
        beta_headers: &beta,
    };
    let authc = AuthCtx {
        store_key: &cfg.provider.name,
        inline_key: cfg.inline_key.as_ref(),
        api_header: cfg.provider.api_header.as_ref(),
        oauth: cfg.provider.oauth.as_ref(),
        ambient: cfg.provider.ambient.as_ref(),
    };
    let mut wire = WireRequest::get(format!("{}{}", ctx.base_url, proto.models_path()));
    // The probe/verb skip `encode`, so the static protocol headers it would stamp —
    // notably Anthropic's REQUIRED `anthropic-version` — must ride here, exactly as
    // `encode` applies `ctx.beta_headers` (a bare GET 400s on `/v1/models` without it).
    for (k, v) in &beta {
        wire.set_header(k, v);
    }
    wire.timeouts = cfg.timeouts();
    auth.apply(&mut wire, &ctx, &authc, store, clock, transport)?;
    let resp = transport.send(wire)?;
    if !(200..300).contains(&resp.status) {
        return Err(http_status_err(resp.status));
    }
    let body = drain(resp.body)?;
    proto.decode_models(&body)
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

/// A non-2xx models response → its run-level error carrying the authoritative status
/// (model-discovery §2): `from_http_status` derives 401/403→Auth/77 and every other
/// 4xx→69 / 5xx→70 from the number, the same table the data plane reads.
fn http_status_err(status: u16) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::from_http_status(status),
        message: format!("models request failed with HTTP {status}"),
        provider_detail: None,
    }
}

/// Drain a 2xx body to end (a small JSON document, not a stream — it bypasses the
/// framers); a mid-collection transport drop is a `Transport` error (→69).
fn drain(
    body: Box<dyn Iterator<Item = std::io::Result<Vec<u8>>>>,
) -> Result<Vec<u8>, CanonicalError> {
    let mut buf = Vec::new();
    for chunk in body {
        match chunk {
            Ok(c) => buf.extend_from_slice(&c),
            Err(e) => {
                return Err(CanonicalError {
                    kind: ErrorKind::Transport,
                    message: format!("failed to read models response body: {e}"),
                    provider_detail: None,
                })
            }
        }
    }
    Ok(buf)
}

/// A stdout write failure for the listing → `Transport` (→69), the verb's pre-sink
/// analogue of the data plane's `BrokenPipe`/write handling.
fn write_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to write model list: {e}"),
        provider_detail: None,
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
            },
            Model {
                id: "smart".into(),
                default: true,
            },
        ];
        let mut out = Vec::new();
        print_models(&mut out, &models, false).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "fast\nsmart (default)\n");
    }
}
