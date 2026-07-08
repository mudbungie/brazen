//! The one array-of-tables (`[[provider]]`) ⇄ keyed-map seam (config §2.2): the
//! custom `Deserialize` for [`PartialConfig`]. A `[[provider]]` row carries its
//! own `name` (the map key — single source of truth), so it cannot be a flatten;
//! `deny_unknown_fields` on the row makes a typo'd key a `MalformedFile` (§2.3).
//! An unmodeled top-level key lands in `extra` rather than erroring — the one
//! sanctioned long-tail valve.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::auth::OAuthConfig;
use crate::config::partial::{PartialConfig, PartialProvider};
use crate::config::provider::{AuthId, HeaderSpec, ModelsOverride, ProtocolId};
use crate::store::AmbientSpec;

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
    model_prefixes: Option<Vec<String>>,
    #[serde(default)]
    body_defaults: Map<String, Value>,
    unsupported_body_keys: Option<Vec<String>>,
    models: Option<ModelsOverride>,
    oauth: Option<OAuthConfig>,
    ambient: Option<AmbientSpec>,
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
                model_prefixes: self.model_prefixes,
                body_defaults: self.body_defaults,
                unsupported_body_keys: self.unsupported_body_keys,
                models: self.models,
                oauth: self.oauth,
                ambient: self.ambient,
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
                        // The FIRST declared row is this layer's zero-config default
                        // (config-file order — config §4.3); the keyed map below loses
                        // that order, so capture it before consuming the rows.
                        if let Some(first) = rows.first() {
                            cfg.default_provider = Some(first.name.clone());
                        }
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
                // A TOP-LEVEL `base_url` is the host-override scalar (config §4.5), NOT a
                // provider row's `base_url` (that rides inside a `[[provider]]` table); the
                // two keys never collide, so a file can carry both.
                "base_url" => cfg.base_url = Some(map.next_value()?),
                "api_key" => cfg.api_key = Some(map.next_value()?),
                "output" => cfg.output = Some(map.next_value()?),
                "thinking" => cfg.thinking = Some(map.next_value()?),
                "max_tokens" => cfg.max_tokens = Some(map.next_value()?),
                "temperature" => cfg.temperature = Some(map.next_value()?),
                "top_p" => cfg.top_p = Some(map.next_value()?),
                "reasoning" => cfg.reasoning = Some(map.next_value()?),
                "stream" => cfg.stream = Some(map.next_value()?),
                "timeout" => cfg.timeout = Some(map.next_value()?),
                "system" => cfg.system = Some(map.next_value()?),
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
