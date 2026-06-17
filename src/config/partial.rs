//! The one config schema (config §2). Flags, env, file, and embedded defaults
//! are four instances of `PartialConfig`, every field `Option`, every provider
//! entry sparse — `None` is the identity of `Option::or`, so a missing layer
//! contributes nothing and "is this set?" needs no second flag (config §2.1).
//! `or` is the single associative fold step, identical for scalars and the
//! provider table (config §3.1, §3.2). The custom `Deserialize` — the one
//! array-of-tables (`[[provider]]`) ⇄ keyed-map seam (config §2.2) — lives in
//! the sibling [`partial_de`](super::partial_de).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::auth::OAuthConfig;
use crate::canonical::Content;
use crate::config::provider::{AuthId, HeaderSpec, ProtocolId};
use crate::store::Secret;

/// The output projection (arch §5.1): `--text` default, `--json` → `Ndjson`,
/// `--raw` → `Raw`. The single enum behind both `PartialConfig.output` and
/// `ResolvedConfig.output` — one home for "which projection" (config §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutMode {
    Text,
    Ndjson,
    Raw,
}

impl OutMode {
    /// Parse a config/env spelling (`text`/`ndjson`/`raw`) — `None` for an
    /// unrecognized value, lifted to a `BadValue` by the caller (config §7).
    pub fn parse(s: &str) -> Option<OutMode> {
        match s {
            "text" => Some(OutMode::Text),
            "ndjson" => Some(OutMode::Ndjson),
            "raw" => Some(OutMode::Raw),
            _ => None,
        }
    }
}

/// A sparse provider row: every `Provider` field made `Option` so a file can
/// patch ONE field of an embedded row without redeclaring it (config §3.2).
/// `name` is absent — it is the map key (single source of truth).
#[derive(Default, Clone, Debug, PartialEq, serde::Serialize)]
pub struct PartialProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
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
    /// The row's request-body defaults (config §4.1): gen params fold into the
    /// resolved request, the rest ride `req.extra` — the row's own long-tail valve.
    /// Merged per-key under `or_map`, like the top-level `extra` (config §3.2).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub body_defaults: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthConfig>,
}

impl PartialProvider {
    /// `self` (higher precedence) wins per field; `None` defers (config §3.2).
    fn or(self, other: PartialProvider) -> PartialProvider {
        PartialProvider {
            base_url: self.base_url.or(other.base_url),
            protocol: self.protocol.or(other.protocol),
            auth: self.auth.or(other.auth),
            api_header: self.api_header.or(other.api_header),
            beta_headers: self.beta_headers.or(other.beta_headers),
            model_aliases: self.model_aliases.or(other.model_aliases),
            body_defaults: or_map(self.body_defaults, other.body_defaults),
            oauth: self.oauth.or(other.oauth),
        }
    }
}

/// The one config type (config §2). Built four times — flags, env, file,
/// defaults — and folded under `or`. `provider` is the selected provider name;
/// `providers` is the sparse row table (wire key `[[provider]]`, §2.2).
#[derive(Default, Clone, Debug, PartialEq)]
pub struct PartialConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<Secret>,
    pub output: Option<OutMode>,
    /// `--thinking`: emit reasoning before the answer under the text projection
    /// (arch §5.3). A flag on text mode, not a fourth `OutMode` — inert outside it.
    pub thinking: Option<bool>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stream: Option<bool>,
    /// Per-request transport timeouts in WHOLE SECONDS (config §4): `connect`
    /// caps connection establishment, `response` caps awaiting the response
    /// headers, and `idle` is the inter-chunk bound on the streaming body
    /// (`data/defaults.toml` carries the floor). `None` defers like any scalar.
    pub timeout_connect: Option<u64>,
    pub timeout_response: Option<u64>,
    pub timeout_idle: Option<u64>,
    /// The leading, config-/flag-/file-sourced system prompt (arch §3.1, §4.4,
    /// Decision 10): the ergonomic "data transported by bz", filled into a request
    /// that omits its own `system`. Distinct from a `Role::System` transcript
    /// message — position is the distinguishing fact, not a second home.
    pub system: Option<Vec<Content>>,
    pub providers: BTreeMap<String, PartialProvider>,
    pub extra: Map<String, Value>,
}

impl PartialConfig {
    /// The fold step: `self` outranks `other`. Every scalar is `Option::or`;
    /// the provider table merges per-key, per-field; the `extra` map lets the
    /// higher-precedence key win. `or` is associative, so the four-layer fold
    /// needs no parenthesization (config §3.1).
    pub fn or(self, other: PartialConfig) -> PartialConfig {
        PartialConfig {
            provider: self.provider.or(other.provider),
            model: self.model.or(other.model),
            api_key: self.api_key.or(other.api_key),
            output: self.output.or(other.output),
            thinking: self.thinking.or(other.thinking),
            max_tokens: self.max_tokens.or(other.max_tokens),
            temperature: self.temperature.or(other.temperature),
            top_p: self.top_p.or(other.top_p),
            stream: self.stream.or(other.stream),
            timeout_connect: self.timeout_connect.or(other.timeout_connect),
            timeout_response: self.timeout_response.or(other.timeout_response),
            timeout_idle: self.timeout_idle.or(other.timeout_idle),
            system: self.system.or(other.system),
            providers: merge_providers(self.providers, other.providers),
            extra: or_map(self.extra, other.extra),
        }
    }
}

/// Union of keys; a key in both layers merges field-by-field under the same
/// `or` (config §3.2) — the SAME mechanism that folds scalars, no second
/// merge algorithm.
fn merge_providers(
    mut hi: BTreeMap<String, PartialProvider>,
    lo: BTreeMap<String, PartialProvider>,
) -> BTreeMap<String, PartialProvider> {
    for (key, lo_row) in lo {
        let merged = match hi.remove(&key) {
            Some(hi_row) => hi_row.or(lo_row),
            None => lo_row,
        };
        hi.insert(key, merged);
    }
    hi
}

/// The `extra` valve folds like everything else: the higher-precedence key
/// wins, a key only in the lower layer passes through. Shared by the top-level
/// `extra`, a row's `body_defaults`, and the resolve-time merge of a row's
/// non-gen `body_defaults` over the top-level `extra` (config §3.2, §4.1).
pub(crate) fn or_map(mut hi: Map<String, Value>, lo: Map<String, Value>) -> Map<String, Value> {
    for (key, value) in lo {
        hi.entry(key).or_insert(value);
    }
    hi
}
