//! The resolved config the pipeline runs on, and `fill_absent` (config §4, §7).
//! `model` is the alias-resolved WIRE id, so `ProviderCtx.model` is final and
//! `encode` has no model logic (arch §4.1). The gen scalars already fold the
//! routed row's `body_defaults` beneath flag/env/file at resolve (config §4.1),
//! so `fill_absent` is a plain `Option::or` per field — no per-field query here.

use serde_json::{Map, Value};

use crate::canonical::{CanonicalRequest, Content};
use crate::config::partial::OutMode;
use crate::config::provider::Provider;
use crate::store::Secret;
use crate::transport::Timeouts;

/// The one config the pipeline runs on (config §7). `model` is the alias-
/// resolved WIRE id, so `ProviderCtx.model` is final and `encode` has no model
/// logic (arch §4.1). Each gen scalar already carries the routed row's
/// `body_defaults` beneath flag/env/file (folded at resolve, config §4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConfig {
    pub provider: Provider,
    pub model: String,
    pub output: OutMode,
    /// `--thinking` resolved to a concrete bool (default `false`); the text sink
    /// reads it to gate reasoning + the separator (arch §5.3). Inert in NDJSON/raw.
    pub thinking: bool,
    pub inline_key: Option<Secret>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stream: Option<bool>,
    /// The resolved per-request transport timeouts in seconds (config §4): each
    /// `None` leaves that bound unset. `bz` reads them via [`Self::timeouts`] and
    /// stamps the `WireRequest`; the floor is `data/defaults.toml`.
    pub timeout_connect: Option<u64>,
    pub timeout_response: Option<u64>,
    pub timeout_idle: Option<u64>,
    /// The resolved leading system prompt (config §4, §7): `fill_absent` supplies
    /// it to a request that omits its own `system`. `None` is the no-system path.
    pub system: Option<Vec<Content>>,
    /// Config-level body passthrough (config §4.1): the top-level `extra` map with
    /// the routed row's non-gen `body_defaults` merged over it (the row wins, being
    /// more specific). `fill_absent` seeds it into `req.extra` beneath the request's
    /// own keys, so it reaches the wire through the encoders' one `req.extra` fold.
    pub extra: Map<String, Value>,
}

impl ResolvedConfig {
    /// The resolved transport timeouts as the seam's [`Timeouts`] (config §4): a
    /// query that projects the three scalars onto the record `run` stamps on the
    /// `WireRequest`, so "which bounds" has one home — the resolved config.
    pub fn timeouts(&self) -> Timeouts {
        Timeouts {
            connect: self.timeout_connect,
            response: self.timeout_response,
            idle: self.timeout_idle,
        }
    }
}

/// Fill each gen field the request OMITS from config; request-present fields are
/// untouched (config §4). So per field the effective order is
/// request > flag > env > config > row body_default, by composition — never one
/// fold the caller must learn (each `cfg` gen scalar already folds the row default,
/// config §4.1). Structural payload (messages/tools) is the request's alone; the
/// request's OWN `extra` keys win, and config passthrough (`cfg.extra`) seeds only
/// the keys the request left unset (arch §4.4, config §4.1).
pub fn fill_absent(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    if req.model.is_empty() {
        req.model = cfg.model.clone();
    }
    req.max_tokens = req.max_tokens.or(cfg.max_tokens);
    req.temperature = req.temperature.or(cfg.temperature);
    req.top_p = req.top_p.or(cfg.top_p);
    req.stream = req.stream.or(cfg.stream);
    req.system = req.system.take().or_else(|| cfg.system.clone());
    for (k, v) in &cfg.extra {
        req.extra.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Drop the request-body fields the routed backend cannot accept — the inverse of
/// `body_defaults` (config §4.1). Some backends 400 on a param brazen would
/// otherwise forward (the Codex `…/codex/responses` backend rejects `temperature`,
/// `top_p`, and `max_output_tokens` as "Unsupported parameter"). Run AFTER
/// `fill_absent`, so a field is cleared whatever its source — request, flag, env,
/// or row default. The keys name CANONICAL fields (like `body_defaults`), so the
/// canonical→wire rename (`max_tokens`→`max_output_tokens`) stays owned by `encode`;
/// a non-gen key falls through to the `extra` valve. Silent, mirroring the always-
/// stream force in `serve`: brazen normalizes to what the provider accepts.
pub fn strip_unsupported(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    for key in &cfg.provider.unsupported_body_keys {
        match key.as_str() {
            "max_tokens" => req.max_tokens = None,
            "temperature" => req.temperature = None,
            "top_p" => req.top_p = None,
            other => {
                req.extra.remove(other);
            }
        }
    }
}

/// Ensure the resolved system LEADS with the auth mode's required preamble (auth
/// §4.1). An OAuth row may mandate a leading system block — a Claude-Code-scoped
/// Anthropic OAuth token rejects a request whose system does not begin with the
/// Claude-Code line. It is a BODY fact, so it cannot ride header-only `Auth::apply`
/// (which runs on the already-encoded `WireRequest`); resolution prepends it here,
/// AFTER [`fill_absent`] and BEFORE `encode`, and every protocol's existing
/// `req.system` projection carries it (arch §3.1) — one site, no per-encoder code.
///
/// The invariant is "the system leads with the preamble", not "the preamble was
/// prepended": idempotent, so a re-fed transcript already leading with the line is
/// left untouched (no double). A row with no `system_preamble`, or any non-OAuth
/// row, is the empty case — `req.system` is left exactly as `fill_absent` left it
/// (incl. `None`), the general path with empty input, not a special case.
pub fn lead_with_preamble(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    let Some(text) = cfg
        .provider
        .oauth
        .as_ref()
        .and_then(|o| o.system_preamble.as_deref())
    else {
        return;
    };
    let lead = Content::Text(text.to_owned());
    let system = req.system.get_or_insert_with(Vec::new);
    if system.first() != Some(&lead) {
        system.insert(0, lead);
    }
}
