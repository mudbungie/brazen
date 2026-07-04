//! The wire repr of [`Tool`](super::Tool) (CR-4): hand-rolled serde keyed on the
//! PRESENCE of the `"type"` key — an object without one is `Custom` (today's
//! custom-tool shape, projected across dialects), an object with one is
//! `Provider` (opaque `kind` + config, carried verbatim to the routed provider).
//! `config` captures EVERY key except `type`/`name`, so unknown provider config
//! (max_uses, allowed_domains, user_location, …) survives the round-trip. Kept
//! beside `request_de`, mirroring the model/wire split.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::request::Tool;

/// The `Custom` wire shape: `{name, description?, input_schema}` — `description`
/// omitted when `None` (matches the pre-enum wire bytes exactly).
#[derive(Deserialize)]
struct CustomWire {
    name: String,
    #[serde(default)]
    description: Option<String>,
    input_schema: Value,
}

/// The `Provider` wire shape: `{"type": kind, "name": name, ...config}` — the
/// config keys flattened as siblings; no `input_schema`, no `description`.
#[derive(Deserialize)]
struct ProviderWire {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(flatten)]
    config: Map<String, Value>,
}

impl Serialize for Tool {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Borrowed mirrors of the two wire shapes — serialize without cloning.
        #[derive(Serialize)]
        struct Custom<'a> {
            name: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: &'a Option<String>,
            input_schema: &'a Value,
        }
        #[derive(Serialize)]
        struct Provider<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            name: &'a str,
            #[serde(flatten)]
            config: &'a Map<String, Value>,
        }
        match self {
            Tool::Custom {
                name,
                description,
                input_schema,
            } => Custom {
                name,
                description,
                input_schema,
            }
            .serialize(s),
            Tool::Provider { kind, name, config } => Provider { kind, name, config }.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        // The dispatch: a `type` key means provider-typed; its absence means the
        // caller-defined custom shape (absent `description` stays `None`, absent
        // `input_schema` is an input error — the pre-enum strictness, preserved).
        if v.get("type").is_some() {
            let w = ProviderWire::deserialize(v).map_err(de::Error::custom)?;
            Ok(Tool::Provider {
                kind: w.kind,
                name: w.name,
                config: w.config,
            })
        } else {
            let w = CustomWire::deserialize(v).map_err(de::Error::custom)?;
            Ok(Tool::Custom {
                name: w.name,
                description: w.description,
                input_schema: w.input_schema,
            })
        }
    }
}
