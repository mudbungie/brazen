//! Model discovery (model-discovery §2, §5): the `bz --list-models` control flag — the
//! SOLE writer of the model cache and the ONLY model-list fetch in `bz` (the generation
//! path reads the cache this verb wrote, never GETs `/models`). [`fetch_models`] is
//! the verb's "GET `{base_url}{models_path}`, auth, drain the 2xx body, decode";
//! after a successful decode the verb prints the list AND writes it to the cache
//! (`cache.put`, best-effort). The GET carries the row's `beta_headers` (e.g.
//! Anthropic's required `anthropic-version`) exactly as `encode` does, since it skips
//! `encode`.

use std::io::Write;

use crate::auth::encode_pairs;
use crate::canonical::{CanonicalError, ErrorKind, Model};
use crate::config::provider::ModelsOverride;
use crate::config::{
    config_path, defaults, partial_from_env, read_config_file, OutMode, ResolvedConfig,
};
use crate::protocol::{decode_models, http_error, ModelsShape, WireRequest};
use crate::registry::Registry;
use crate::store::{Clock, CredStore, ModelCache};
use crate::transport::Transport;

use super::drain;
use super::events::is_2xx;

/// The injected seams + writers for one `bz --list-models` (model-discovery §2), the
/// sibling of `LoginIo`. The verb writes its listing to `stdout` and any error to
/// `stderr`, reuses the data-plane `Transport`/`CredStore`/`Clock` for the one GET
/// (auth/refresh and all, through the same `Auth::apply` seam), and is the SOLE writer
/// of the `cache` — it `put`s the decoded list the generation path later reads (§5).
pub struct ListIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub transport: &'a dyn Transport,
    pub store: &'a dyn CredStore,
    pub cache: &'a dyn ModelCache,
    pub clock: &'a dyn Clock,
}

/// Run `bz --list-models` and return the POSIX exit code (model-discovery §2). Reuses
/// the full flag parser + `into_resolved(None)` to pick the provider (an explicit
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
    if flags.version {
        return Ok(super::emit(io.stdout, super::VERSION_LINE));
    }
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    let cfg: ResolvedConfig = merged.into_resolved(None).map_err(CanonicalError::from)?;
    // The verb's output shape is the SAME resolved fact the data plane folds (run::run),
    // not the flag layer alone: `--json`, `BRAZEN_OUTPUT=ndjson`, and a config-file
    // `output = "ndjson"` all select the object form, exactly as they do for generation.
    let json = cfg.output == OutMode::Ndjson;
    let models = fetch_models(&cfg, io.transport, io.store, io.clock)?;
    // Write the cache — the SOLE write site (model-discovery §5). Best-effort: `put` is
    // atomic + warns on its own IO failure (the impl's concern), so the verb's exit is
    // exactly the listing's, never the cache write's. The generation path reads this.
    io.cache.put(&cfg.provider.name, &models);
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

/// The verb's models-list round-trip (model-discovery §5) — the ONLY model-list fetch
/// in `bz`: GET `{base_url}{models_path}`, stamp the resolved timeouts, `Auth::apply`
/// (the same seam — api-key/bearer/oauth, refresh and all), send, drain the WHOLE 2xx
/// body, and `decode_models`. A non-2xx maps through `from_http_status` carrying the
/// status (4xx→69/auth-77, 5xx→70); a malformed 2xx body is the `Provider{502}`
/// `decode_models` raises. The GET carries the row's `beta_headers` because it skips
/// `encode`, which is where the generation path otherwise stamps them.
fn fetch_models(
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
    let ctx = cfg.provider_ctx(&beta);
    let authc = cfg.auth_ctx();
    // The effective discovery request: the protocol's `ModelsShape` defaults OVERLAID
    // per key by the row's optional `[provider.models]` (model-discovery §3.2). One
    // pure helper, no per-row branch — a row with no override yields the plain
    // protocol-default `{base_url}{path}` URL and the protocol's default decode keys.
    let req = models_req(
        proto.models_shape(),
        cfg.provider.models.as_ref(),
        ctx.base_url,
    );
    let mut wire = WireRequest::get(req.url);
    // The verb skips `encode`, so the static protocol headers it would stamp —
    // notably Anthropic's REQUIRED `anthropic-version` — must ride here, exactly as
    // `encode` applies `ctx.beta_headers` (a bare GET 400s on `/v1/models` without it).
    for (k, v) in &beta {
        wire.set_header(k, v);
    }
    wire.timeouts = cfg.timeouts();
    auth.apply(&mut wire, &ctx, &authc, store, clock, transport)?;
    let resp = transport.send(wire)?;
    let status = resp.status;
    if !is_2xx(status) {
        // Carry the provider's diagnostic, exactly as the data plane does: drain the
        // non-2xx body and route it through the ONE `http_error` home, so the verb
        // surfaces the status-driven `kind` AND the raw body in `provider_detail`
        // / `message` (a 400 `missing anthropic-version`, a 401 hint, …) — never a
        // bespoke "HTTP {status}" that throws the body away (model-discovery §2). A
        // mid-collection drop yields no body, so the authoritative status alone drives
        // it (an empty body degrades to message/`None`).
        let body = drain(resp.body).unwrap_or_default();
        return Err(http_error(&body, status));
    }
    let body = drain(resp.body).map_err(read_failed)?;
    // The ONE generic decoder, fed the effective keys (model-discovery §3): the
    // protocol default `array_key`/`id_key` overridden per row, `strip` protocol-only.
    decode_models(&body, req.array_key, req.id_key, req.strip)
}

/// The effective models-discovery request: the protocol's [`ModelsShape`] defaults
/// OVERRIDDEN per row by `[provider.models]` (model-discovery §3.2). PURE.
/// `path`/`array_key`/`id_key` fall back to the protocol default when the row omits
/// them (the inherit rule — less config); `query` is the row's (empty by default);
/// `strip` is protocol-only, never row-overridable. The URL is `{base_url}{path}` plus
/// a `?`-query ONLY when the row pins one — percent-encoded by the OAuth [`encode_pairs`]
/// codec (reused, not reinvented) — so a default-shape row's URL is byte-for-byte the
/// pre-override `{base_url}{path}`.
pub(crate) fn models_req<'a>(
    shape: ModelsShape,
    over: Option<&'a ModelsOverride>,
    base_url: &str,
) -> ModelsReq<'a> {
    let path = over.and_then(|m| m.path.as_deref()).unwrap_or(shape.path);
    let array_key = over
        .and_then(|m| m.array_key.as_deref())
        .unwrap_or(shape.array_key);
    let id_key = over
        .and_then(|m| m.id_key.as_deref())
        .unwrap_or(shape.id_key);
    let query = over.map(|m| m.query.as_slice()).unwrap_or(&[]);
    let url = if query.is_empty() {
        format!("{base_url}{path}")
    } else {
        let pairs: Vec<(&str, &str)> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        format!("{base_url}{path}?{}", encode_pairs(&pairs))
    };
    ModelsReq {
        url,
        array_key,
        id_key,
        strip: shape.strip,
    }
}

/// The resolved discovery request facts (URL + decode keys) [`models_req`] computes —
/// the keys borrow either the `&'static` protocol shape or the `'a` row override.
pub(crate) struct ModelsReq<'a> {
    pub(crate) url: String,
    pub(crate) array_key: &'a str,
    pub(crate) id_key: &'a str,
    pub(crate) strip: &'a str,
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

/// A mid-collection transport drop while draining the 2xx body → `Transport` (→69),
/// CARRYING the `io::Error` so the failure stays diagnosable. The shared
/// [`drain`](super::drain) is the one collection home (it bypasses the framers — a
/// small JSON document, not a stream); `models` maps its `io::Error` here, the
/// `respond` side maps it to an in-band `Transport` event.
fn read_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to read models response body: {e}"),
        provider_detail: None,
    }
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
