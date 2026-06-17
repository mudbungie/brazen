//! The resolved config the pipeline runs on, and `fill_absent` (config §4, §7).
//! `model` is the alias-resolved WIRE id, so `ProviderCtx.model` is final and
//! `encode` has no model logic (arch §4.1). `effective_max_tokens` is a query,
//! not a field — never store what you can compute.

use serde_json::{Map, Value};

use crate::canonical::{CanonicalRequest, Content};
use crate::config::partial::OutMode;
use crate::config::provider::Provider;
use crate::store::Secret;
use crate::transport::Timeouts;

/// The one config the pipeline runs on (config §7). `model` is the alias-
/// resolved WIRE id, so `ProviderCtx.model` is final and `encode` has no model
/// logic (arch §4.1). `effective_max_tokens` is a query, not a field.
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
    pub extra: Map<String, Value>,
}

impl ResolvedConfig {
    /// `max_tokens` after the row default: the resolved config value, else the
    /// provider row's `default_max_tokens` at lowest precedence (config §4.1).
    pub fn effective_max_tokens(&self) -> Option<u32> {
        self.max_tokens.or(self.provider.default_max_tokens)
    }

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
/// request > flag > env > config > row-default, by composition — never one fold
/// the caller must learn. Structural payload (messages/tools/extra) is the
/// request's alone and never filled (arch §4.4).
pub fn fill_absent(req: &mut CanonicalRequest, cfg: &ResolvedConfig) {
    if req.model.is_empty() {
        req.model = cfg.model.clone();
    }
    req.max_tokens = req.max_tokens.or_else(|| cfg.effective_max_tokens());
    req.temperature = req.temperature.or(cfg.temperature);
    req.top_p = req.top_p.or(cfg.top_p);
    req.stream = req.stream.or(cfg.stream);
    req.system = req.system.take().or_else(|| cfg.system.clone());
}
