//! The one config schema (config §2). Flags, env, file, and embedded defaults
//! are four instances of `PartialConfig`, every field `Option`, every provider
//! entry sparse — `None` is the identity of `Option::or`, so a missing layer
//! contributes nothing and "is this set?" needs no second flag (config §2.1).
//! `or` is the single associative fold step, identical for scalars and the
//! provider table (config §3.1, §3.2). The custom `Deserialize` is the one
//! array-of-tables (`[[provider]]`) ⇄ keyed-map seam (config §2.2).

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<u32>,
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
            default_max_tokens: self.default_max_tokens.or(other.default_max_tokens),
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
/// wins, a key only in the lower layer passes through.
fn or_map(mut hi: Map<String, Value>, lo: Map<String, Value>) -> Map<String, Value> {
    for (key, value) in lo {
        hi.entry(key).or_insert(value);
    }
    hi
}

/// One `[[provider]]` table on the wire: `name` plus the sparse row fields.
/// `deny_unknown_fields` makes a typo'd row key a parse error → `MalformedFile`
/// (config §2.3, §7); flatten is avoided precisely so the deny can fire.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderRow {
    name: String,
    base_url: Option<String>,
    protocol: Option<ProtocolId>,
    auth: Option<AuthId>,
    beta_headers: Option<Vec<(String, String)>>,
    api_header: Option<HeaderSpec>,
    model_aliases: Option<BTreeMap<String, String>>,
    default_max_tokens: Option<u32>,
}

impl ProviderRow {
    fn into_pair(self) -> (String, PartialProvider) {
        (
            self.name,
            PartialProvider {
                base_url: self.base_url,
                protocol: self.protocol,
                auth: self.auth,
                api_header: self.api_header,
                beta_headers: self.beta_headers,
                model_aliases: self.model_aliases,
                default_max_tokens: self.default_max_tokens,
            },
        )
    }
}

/// The `provider` key is overloaded by value type: a string selects a provider
/// (`provider = "anthropic"`), an array-of-tables defines rows (`[[provider]]`).
/// A single TOML file can carry only one form, so the two never collide.
#[derive(Deserialize)]
#[serde(untagged)]
enum ProviderField {
    Selector(String),
    Rows(Vec<ProviderRow>),
}

impl<'de> Deserialize<'de> for PartialConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(PartialConfigVisitor)
    }
}

struct PartialConfigVisitor;

impl<'de> Visitor<'de> for PartialConfigVisitor {
    type Value = PartialConfig;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a brazen config table")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<PartialConfig, M::Error> {
        let mut cfg = PartialConfig::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "provider" => match map.next_value::<ProviderField>()? {
                    ProviderField::Selector(name) => cfg.provider = Some(name),
                    ProviderField::Rows(rows) => {
                        for row in rows {
                            let (name, partial) = row.into_pair();
                            if cfg.providers.insert(name.clone(), partial).is_some() {
                                return Err(de::Error::custom(format!(
                                    "duplicate provider name `{name}`"
                                )));
                            }
                        }
                    }
                },
                "model" => cfg.model = Some(map.next_value()?),
                "api_key" => cfg.api_key = Some(map.next_value()?),
                "output" => cfg.output = Some(map.next_value()?),
                "thinking" => cfg.thinking = Some(map.next_value()?),
                "max_tokens" => cfg.max_tokens = Some(map.next_value()?),
                "temperature" => cfg.temperature = Some(map.next_value()?),
                "top_p" => cfg.top_p = Some(map.next_value()?),
                "stream" => cfg.stream = Some(map.next_value()?),
                // The one sanctioned long-tail: an unmodeled top-level key lands
                // in `extra` rather than erroring (config §2.3).
                _ => {
                    cfg.extra.insert(key, map.next_value()?);
                }
            }
        }
        Ok(cfg)
    }
}
