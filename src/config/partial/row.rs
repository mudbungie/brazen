//! The sparse provider row (config §3.2): every `Provider` field made `Option` so a
//! file can patch ONE field of an embedded row without redeclaring it, plus the row's
//! own `or` fold step — the SAME per-field `Option::or` that folds the top-level
//! scalars, applied one level down. The complete row it lifts into lives in
//! `config::provider`; the top-level schema and the fold live in the parent module.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::auth::OAuthConfig;
use crate::config::provider::{AuthId, HeaderSpec, ModelsOverride, ProtocolId};
use crate::store::AmbientSpec;

use super::or_map;

/// A sparse provider row: every `Provider` field made `Option` so a file can
/// patch ONE field of an embedded row without redeclaring it (config §3.2).
/// `name` is absent — it is the map key (single source of truth).
#[derive(Default, Clone, Debug, PartialEq, serde::Serialize)]
pub struct PartialProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The subprocess program for an exec-transport dialect (claude-code spec §7.1);
    /// substitutes for `base_url` — a row carrying `exec` may omit `base_url`
    /// (completed as `""` at resolve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProtocolId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_header: Option<HeaderSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_aliases: Option<BTreeMap<String, String>>,
    /// Model-id family prefixes the row OWNS for routing (arch §4.3): the row
    /// claims every model whose id starts with one of these (e.g. anthropic owns
    /// `claude-`), so an unmistakable wire id routes with no `--provider`. Routing
    /// only — substitution stays `model_aliases`'s job; the two feed one query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_prefixes: Option<Vec<String>>,
    /// The row's request-body defaults (config §4.1): gen params fold into the
    /// resolved request, the rest ride `req.extra` — the row's own long-tail valve.
    /// Merged per-key under `or_map`, like the top-level `extra` (config §3.2).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub body_defaults: Map<String, Value>,
    /// Canonical request-body fields this backend cannot accept (config §4.1): the
    /// inverse of `body_defaults`, stripped from the request by `strip_unsupported`
    /// so the encoder never emits them. Whole-list `or` like `beta_headers` — a
    /// higher-precedence layer replaces the list rather than merging keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported_body_keys: Option<Vec<String>>,
    /// The `[provider.models]` discovery override (config §4.4): a whole-block
    /// `Option::or` across layers, like `beta_headers` — a higher-precedence layer
    /// replaces the block rather than merging keys. `None` ⇒ the protocol default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelsOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient: Option<AmbientSpec>,
}

impl PartialProvider {
    /// `self` (higher precedence) wins per field; `None` defers (config §3.2).
    pub(super) fn or(self, other: PartialProvider) -> PartialProvider {
        PartialProvider {
            base_url: self.base_url.or(other.base_url),
            exec: self.exec.or(other.exec),
            protocol: self.protocol.or(other.protocol),
            auth: self.auth.or(other.auth),
            api_header: self.api_header.or(other.api_header),
            beta_headers: self.beta_headers.or(other.beta_headers),
            model_aliases: self.model_aliases.or(other.model_aliases),
            model_prefixes: self.model_prefixes.or(other.model_prefixes),
            body_defaults: or_map(self.body_defaults, other.body_defaults),
            unsupported_body_keys: self.unsupported_body_keys.or(other.unsupported_body_keys),
            models: self.models.or(other.models),
            oauth: self.oauth.or(other.oauth),
            ambient: self.ambient.or(other.ambient),
        }
    }
}
