//! The typed generation core (arch §1, §4.4): a `CanonicalRequest` in, an
//! `Iterator<Item = Event>` out — the pure pipeline minus byte-serialization. `run` is
//! the byte adapter over it (parse stdin → request, fold config, `pump` the events into
//! the sink); an embedder calls it directly and `match`es the typed events. Errors are
//! in-band: a request-half failure (model resolution, encode, auth, transport) yields
//! one `Event::Error` then the terminal `End`, exactly as the streamed path does
//! (§5.9, §8) — so the signature is total, never a `Result` the caller must thread.
//! `--raw` is NOT typed (it never decodes); it lives in [`serve_raw`](super::serve).

use crate::canonical::{select_model, CanonicalError, CanonicalRequest, Event, Model, Provenance};
use crate::config::{fill_absent, lead_with_preamble, strip_unsupported, ResolvedConfig};
use crate::protocol::Protocol;
use crate::registry::Registry;
use crate::transport::TransportResponse;

use super::events::{is_2xx, response_events};
use super::Host;

/// Generate against a resolved config (arch §1): drive ONE round-trip and yield the
/// canonical event stream, terminated by a single `End`. THE pure typed core — `run`
/// wraps it in byte I/O, an embedder consumes the events directly. The model SEED is
/// resolved against the per-provider cache here (a local file read via `host.cache`,
/// model-discovery §5.2), then the request is encoded, authenticated, and sent over the
/// one `Transport`. Every failure is an in-band `Event::Error`, so the call is total.
pub fn generate(
    request: CanonicalRequest,
    config: ResolvedConfig,
    host: &Host,
) -> impl Iterator<Item = Event> {
    let stream: Box<dyn Iterator<Item = Event>> = match build_send(request, config, host) {
        // `host.clock.now()` (the injected `Clock` seam) lets `response_events` parse a
        // `Retry-After` HTTP-date to seconds (§3.3) — never a wall-clock read in the lib.
        Ok((proto, resp, streamed, hint)) => {
            response_events(proto, resp, streamed, hint, host.clock.now())
        }
        Err(e) => Box::new(std::iter::once(Event::Error(e))),
    };
    stream.chain(std::iter::once(Event::End))
}

/// The request half (arch §4.4): resolve the model against the cache, dispatch the
/// protocol/auth over the closed key-enums (no vendor-name match, §4.4), fill/preamble/
/// strip the request, encode, stamp headers/timeouts, authenticate, and send — returning
/// the response, the streaming intent, and the §5.3 model-provenance hint. Any step's
/// error rides back as a `CanonicalError` for `generate` to surface in-band.
fn build_send(
    mut request: CanonicalRequest,
    mut config: ResolvedConfig,
    host: &Host,
) -> Result<
    (
        &'static dyn Protocol,
        TransportResponse,
        bool,
        Option<String>,
    ),
    CanonicalError,
> {
    // Resolve the model SEED against the per-provider cache (model-discovery §5.2): a
    // LOCAL FILE READ, every request, never a round-trip. `select_model` is TOTAL — a
    // hit expands the seed to its wire id (`Cached`), a cold/absent cache passes it
    // verbatim (`Verbatim`); the lone error is an absent model with an empty cache (78).
    let models = host.cache.get(&config.provider.name).unwrap_or_default();
    let (wire_model, prov) = select_model(&models, &config.model, &config.provider.name)?;
    config.model = wire_model;
    config.model_from_cache = matches!(prov, Provenance::Cached);

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

    fill_absent(&mut request, &config);
    // An auth mode may mandate a leading system preamble (auth §4.1) — a BODY fact, so
    // resolution prepends it here and `encode` carries it like any other `req.system`.
    lead_with_preamble(&mut request, &config);
    // Drop fields the routed backend can't accept (config §4.1.1) AFTER the fill, so an
    // explicit --temperature/--top-p/--max-tokens is cleared too (the Codex 400 set).
    strip_unsupported(&mut request, &config);
    // The streaming intent the body carries (architecture §3.2), resolved by `fill_absent`
    // to a concrete bool: a bare request defaults to brazen's stream-native `true`;
    // --no-stream / body_defaults={stream=false} honor `false`. Carried to the fold.
    let streamed = request.stream.unwrap_or(true);

    let mut wire = proto.encode(&request, &ctx)?;
    // content-type + the row's STATIC beta_headers + timeouts, stamped once before auth —
    // the single home for all three (`encode` stays oblivious; auth-DEPENDENT betas ride
    // `auth.apply`), and BEFORE apply so the OAuth refresh's token POST inherits them.
    wire.set_header("content-type", proto.content_type());
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    wire.timeouts = config.timeouts();
    auth.apply(
        &mut wire,
        &ctx,
        &authc,
        host.store,
        host.clock,
        host.transport,
    )?;

    let resp = host.transport.send(wire)?;
    // Learn the model that worked (model-discovery §5.2): a 2xx on a VERBATIM model — one
    // the cache could not place, yet the provider accepted — appends it to this provider's
    // cache, so a later bare `bz` (empty seed) defaults to it and a partial matches it. A
    // Cached model is already in `models`, so ONLY the verbatim-success case writes (no
    // churn, and Verbatim guarantees the id is absent — no dedup needed). This is the data
    // plane's one cache write, the sibling of OAuth refresh's cred write; `list-models`
    // stays the authoritative WHOLESALE writer, this only fills a gap discovery left.
    if is_2xx(resp.status) && !config.model_from_cache {
        let mut learned = models;
        learned.push(Model {
            id: config.model.clone(),
            default: false,
            // A verbatim-learned id carries no provider metadata — the data plane never
            // lists (§5.4); `--list-models` fills the metadata when it REPLACES the list.
            ..Default::default()
        });
        host.cache.put(&config.provider.name, &learned);
    }
    // The §5.3 404 hint, carried by the cache provenance: a Cached model that 404s means
    // a stale cache; a Verbatim one means a cold cache or a typo. `Some` iff this is a
    // 404 — `response_events` appends it to the decoded error's message.
    let hint = (resp.status == 404).then(|| model_hint(&config.model, config.model_from_cache));
    Ok((proto, resp, streamed, hint))
}

/// The §5.3 provenance hint for a 404 on the generation request: a Cached model the
/// provider rejected points at a STALE cache; a Verbatim one (the cache could not place
/// it) at a COLD cache or a typo. The one message-construction home.
fn model_hint(model: &str, from_cache: bool) -> String {
    if from_cache {
        format!(
            "`{model}` was in the cache but the provider rejected it; \
             the cache may be stale — re-run `bz --list-models`"
        )
    } else {
        format!(
            "`{model}` is not in the model cache; \
             run `bz --list-models` to refresh or enable partial matching"
        )
    }
}
