//! The token-count round-trip (architecture §5.10.1): resolve the model against the cache
//! (the SAME `select_model` the data plane runs, READ-only), preprocess the request as the
//! data plane does before encode (fill/preamble/strip), ask the protocol for its count
//! request (`Protocol::count_tokens` — `None` DECLINES with `Config`/78), stamp
//! content-type/betas/timeouts/auth (the tail `serve` applies), send ONE round-trip, drain
//! the 2xx body, and read the token count. The verb half (parse, resolve, print) is the
//! parent module; this is its wire half, the sibling of `models::fetch`.

use crate::canonical::{select_model, CanonicalError, CanonicalRequest, ErrorKind};
use crate::config::{fill_absent, lead_with_preamble, strip_unsupported, ResolvedConfig};
use crate::protocol::{count_from_body, http_error};
use crate::registry::Registry;
use crate::run::{drain, events::is_2xx};

use super::CountIo;

/// The count round-trip (§5.10.1) — the ONE token-count fetch. Resolves the model seed
/// against the per-provider cache (a local read, never a write — this is NOT the discovery
/// path), preprocesses the request exactly as `build_send` does before encode (so the
/// counted request is what `generate` would send), asks the routed protocol for its count
/// request, and — if the dialect HAS a count endpoint — stamps headers/auth and sends. A
/// dialect with no count endpoint (`None`) DECLINES with `Config` (78): a fabricated count
/// is a lie. A non-2xx maps through the ONE `http_error` home (401/403→77, 4xx→69, 5xx→70);
/// a 2xx body with no token count is the `Provider{502}` `count_from_body` raises.
pub(super) fn fetch_count(
    mut request: CanonicalRequest,
    mut config: ResolvedConfig,
    io: &mut CountIo,
) -> Result<u32, CanonicalError> {
    // Resolve the model SEED against the per-provider cache (model-discovery §5.2): a LOCAL
    // FILE READ, no round-trip and NO write — the count op reads the cache the discovery
    // path wrote, exactly as the generation path does, but never learns.
    let cached = io.cache.get(&config.provider.name).unwrap_or_default();
    let (wire_model, _prov) = select_model(&cached, &config.model, &config.provider.name)?;
    config.model = wire_model;

    let registry = Registry::builtin();
    let proto = registry.protocol(config.provider.protocol);
    let auth = registry.auth(config.provider.auth);
    let beta: Vec<(&str, &str)> = config
        .provider
        .beta_headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let ctx = config.provider_ctx(&beta);
    let authc = config.auth_ctx();

    // The SAME request preprocessing the data plane runs before encode (arch §4.4): fill
    // config defaults, lead with the auth preamble, strip unsupported keys — so the counted
    // request is byte-for-byte the shape `generate` would send.
    fill_absent(&mut request, &config);
    lead_with_preamble(&mut request, &config);
    strip_unsupported(&mut request, &config);

    // Endpoint knowledge is DATA on the protocol (§5.10.1): `None` = no count endpoint →
    // DECLINE (Config, 78); `Some(Ok)` the built request; `Some(Err)` an encode failure.
    let count = match proto.count_tokens(&request, &ctx) {
        None => return Err(no_count_endpoint(&config.provider.name)),
        Some(r) => r?,
    };
    let mut wire = count.wire;
    // content-type + the row's STATIC beta headers + timeouts, stamped once before auth —
    // the same tail `serve` applies (the count body skipped `encode`'s header stamping).
    wire.set_header("content-type", proto.content_type());
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    wire.timeouts = config.timeouts();
    auth.apply(&mut wire, &ctx, &authc, io.store, io.clock, io.transport)?;

    let resp = io.transport.send(wire)?;
    let status = resp.status;
    if !is_2xx(status) {
        // Carry the provider's diagnostic through the ONE `http_error` home, as the data
        // plane and `--list-models` do — status-driven `kind` + the raw body verbatim.
        let body = drain(resp.body).unwrap_or_default();
        return Err(http_error(&body, status));
    }
    let body = drain(resp.body).map_err(read_failed)?;
    count_from_body(&body, count.token_key)
}

/// A provider with no count endpoint → `Config` (78): the DECLINE arm (§5.10.1, §8). Names
/// the provider and points the caller back to its own estimate — brazen refuses to
/// fabricate a count (the Usage-Option-not-zero principle applied to counting, §3.2).
fn no_count_endpoint(provider: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: format!(
            "provider `{provider}` has no token-count endpoint; \
             `--count-tokens` declines rather than fabricate a count — use your own estimate"
        ),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// A mid-collection transport drop while draining the 2xx count body → `Transport` (→69),
/// carrying the `io::Error` so the failure stays diagnosable (the sibling of `models`'s).
fn read_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to read token-count response body: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
