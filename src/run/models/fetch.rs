//! The models-list round-trip (model-discovery §3, §5): the effective discovery
//! request — the protocol's `ModelsShape` defaults overlaid per key by the row's
//! optional `[provider.models]` — and the ONE GET that sends it, drains the 2xx
//! body, and decodes the ids. The verb half (flag parse, resolve, print, the cache
//! write) stays in the parent module; this is its wire half.

use crate::auth::encode_pairs;
use crate::canonical::{CanonicalError, ErrorKind, Model};
use crate::config::provider::ModelsOverride;
use crate::config::ResolvedConfig;
use crate::protocol::{decode_models, http_error, ModelKeys, ModelsShape, WireRequest};
use crate::registry::Registry;
use crate::run::{drain, events::is_2xx};
use crate::store::{Clock, CredStore};
use crate::transport::Transport;

/// The verb's models-list round-trip (model-discovery §5) — the ONLY model-list fetch
/// in `bz`: GET `{base_url}{models_path}`, stamp the resolved timeouts, `Auth::apply`
/// (the same seam — api-key/bearer/oauth, refresh and all), send, drain the WHOLE 2xx
/// body, and `decode_models`. A non-2xx maps through `from_http_status` carrying the
/// status (4xx→69/auth-77, 5xx→70); a malformed 2xx body is the `Provider{502}`
/// `decode_models` raises. The GET carries the row's `beta_headers` because it skips
/// `encode`, which is where the generation path otherwise stamps them.
pub(super) fn fetch_models(
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
    // The honest decline (model-discovery §3, claude-code spec §7.2): a dialect with
    // NO models listing returns `None` — the verb fails with the next move in the
    // message instead of fabricating an endpoint. A row's `[provider.models]` override
    // cannot conjure a listing over it (there is nothing to point it at).
    let Some(shape) = proto.models_shape() else {
        return Err(no_listing(&cfg.provider.name));
    };
    // The effective discovery request: the protocol's `ModelsShape` defaults OVERLAID
    // per key by the row's optional `[provider.models]` (model-discovery §3.2). One
    // pure helper, no per-row branch — a row with no override yields the plain
    // protocol-default `{base_url}{path}` URL and the protocol's default decode keys.
    let req = models_req(shape, cfg.provider.models.as_ref(), ctx.base_url);
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
    // The ONE generic decoder, fed the effective keys (model-discovery §3): the protocol
    // default `array_key`/`id_key` + metadata keys overridden per row, `strip` protocol-only.
    decode_models(&body, &req.keys)
}

/// The effective models-discovery request: the protocol's [`ModelsShape`] defaults
/// OVERRIDDEN per row by `[provider.models]` (model-discovery §3.2). PURE. `path` and
/// the overridable [`ModelKeys`] (`array_key`/`id_key` + the metadata keys) fall back to
/// the protocol default when the row omits them (the inherit rule — less config); `query`
/// is the row's (empty by default); `strip` is protocol-only, never row-overridable. The
/// URL is `{base_url}{path}` plus a `?`-query ONLY when the row pins one — percent-encoded
/// by the OAuth [`encode_pairs`] codec (reused, not reinvented) — so a default-shape row's
/// URL is byte-for-byte the pre-override `{base_url}{path}`.
pub(crate) fn models_req<'a>(
    shape: ModelsShape,
    over: Option<&'a ModelsOverride>,
    base_url: &str,
) -> ModelsReq<'a> {
    let d = shape.keys;
    let pick = |o: Option<&'a String>, def: &'a str| o.map(String::as_str).unwrap_or(def);
    let path = over.and_then(|m| m.path.as_deref()).unwrap_or(shape.path);
    let keys = ModelKeys {
        array_key: pick(over.and_then(|m| m.array_key.as_ref()), d.array_key),
        id_key: pick(over.and_then(|m| m.id_key.as_ref()), d.id_key),
        strip: d.strip, // protocol-only, never row-overridable (§3)
        context_key: pick(over.and_then(|m| m.context_key.as_ref()), d.context_key),
        max_output_key: pick(
            over.and_then(|m| m.max_output_key.as_ref()),
            d.max_output_key,
        ),
        display_name_key: pick(
            over.and_then(|m| m.display_name_key.as_ref()),
            d.display_name_key,
        ),
    };
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
    ModelsReq { url, keys }
}

/// The resolved discovery request facts (URL + decode [`ModelKeys`]) [`models_req`]
/// computes — the keys borrow either the `&'static` protocol shape or the `'a` row override.
pub(crate) struct ModelsReq<'a> {
    pub(crate) url: String,
    pub(crate) keys: ModelKeys<'a>,
}

/// The dialect-has-no-listing decline (model-discovery §3, claude-code spec §7.2):
/// `Config` (→78), the same family as `NoProvider` — a config-level "this row cannot
/// do that", with the caller's next move in the message (learn-on-success fills the
/// cache forward, model-discovery §5.4).
fn no_listing(provider: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: format!(
            "provider `{provider}` has no models listing; pass --model verbatim — \
             a model that succeeds is learned into the cache"
        ),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// A mid-collection transport drop while draining the 2xx body → `Transport` (→69),
/// CARRYING the `io::Error` so the failure stays diagnosable. The shared
/// [`drain`](crate::run::drain) is the one collection home (it bypasses the framers — a
/// small JSON document, not a stream); `models` maps its `io::Error` here, the
/// `respond` side maps it to an in-band `Transport` event.
fn read_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to read models response body: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
