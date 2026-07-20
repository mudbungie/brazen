//! `--dump-config` — the one bridge between flag-encoding and file-encoding
//! (config §6). A config file IS a `PartialConfig` in TOML; flags are the same
//! fact in another encoding. The dump is the SAME fold as resolution MINUS the
//! defaults operand (so a later brazen's better default still reaches the user),
//! with secrets elided to the inert `"<redacted>"` sentinel. Output is
//! deterministic — rows in `providers` (declaration) order, scalar maps in
//! `BTreeMap` order, and a `toml::Value` round-trip give a byte-stable golden
//! (config §6, §8).

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde::Serialize as DeriveSerialize;

use crate::config::env::partial_from_env;
use crate::config::errors::ConfigError;
use crate::config::partial::{PartialConfig, PartialIngress, PartialProvider};
use crate::config::EnvSnapshot;
use crate::store::Secret;

/// The inert sentinel a secret elides to — never a real key, never a `${VAR}`
/// reference. Re-loading it yields an invalid credential, forcing env/store to
/// supply the real secret: a config file is never where a secret lives (§6).
const REDACTED: &str = "<redacted>";

/// Serialize the merged config (config §6). The defaults operand is omitted and
/// secrets are redacted first. The result re-parses identically — the encoding
/// round-trips (config §2.2).
pub fn dump_config(
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
) -> Result<String, ConfigError> {
    let env = partial_from_env(env)?;
    let merged = redact(flags.or(env).or(file));
    // Through a `toml::Value` first so the serializer orders scalars before the
    // `[[provider]]` tables regardless of field order; serialization of our own
    // null-free `PartialConfig` is then infallible (config §6).
    #[allow(clippy::expect_used)]
    let value = toml::Value::try_from(&merged).expect("a redacted PartialConfig is TOML-encodable");
    #[allow(clippy::expect_used)]
    Ok(toml::to_string(&value).expect("a toml::Value is infallibly serializable"))
}

/// Replace each present secret — `api_key`, the `[ingress]` token — with the
/// inert sentinel BEFORE serialization (config §6). No other field bears one.
pub fn redact(mut cfg: PartialConfig) -> PartialConfig {
    if cfg.api_key.is_some() {
        cfg.api_key = Some(Secret::new(REDACTED));
    }
    if let Some(ingress) = cfg.ingress.as_mut() {
        if ingress.token.is_some() {
            ingress.token = Some(Secret::new(REDACTED));
        }
    }
    cfg
}

/// Widen an `f32` to `f64` through its shortest decimal so the dump reads
/// `0.9`, not `0.8999999…`, and still reparses to the same `f32` (config §6).
fn clean_f32(v: f32) -> f64 {
    v.to_string().parse().unwrap_or(v as f64)
}

/// One `[[provider]]` table for the dump: the row-list entry with `name` re-
/// injected beside the sparse row's present fields, the inverse of the
/// deserialize seam (config §2.2).
#[derive(DeriveSerialize)]
struct Row<'a> {
    name: &'a str,
    #[serde(flatten)]
    inner: &'a PartialProvider,
}

impl Serialize for PartialConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if let Some(v) = &self.model {
            m.serialize_entry("model", v)?;
        }
        // The top-level host-override scalar (config §4.5): a bare `base_url` key,
        // distinct from a `[[provider]]` row's `base_url`; the dump shows the merged
        // value so `--dump-config` reflects a `--base-url`/`BRAZEN_BASE_URL` override.
        if let Some(v) = &self.base_url {
            m.serialize_entry("base_url", v)?;
        }
        if let Some(v) = &self.api_key {
            m.serialize_entry("api_key", v.expose())?;
        }
        if let Some(v) = &self.output {
            m.serialize_entry("output", v)?;
        }
        if let Some(v) = &self.thinking {
            m.serialize_entry("thinking", v)?;
        }
        if let Some(v) = &self.max_tokens {
            m.serialize_entry("max_tokens", v)?;
        }
        if let Some(v) = &self.temperature {
            m.serialize_entry("temperature", &clean_f32(*v))?;
        }
        if let Some(v) = &self.top_p {
            m.serialize_entry("top_p", &clean_f32(*v))?;
        }
        if let Some(v) = &self.reasoning {
            m.serialize_entry("reasoning", v)?;
        }
        if let Some(v) = &self.stream {
            m.serialize_entry("stream", v)?;
        }
        if let Some(v) = &self.timeout {
            m.serialize_entry("timeout", v)?;
        }
        if let Some(v) = &self.system {
            m.serialize_entry("system", v)?;
        }
        // The `provider` key is one or the other (a TOML file can hold only one
        // shape): the row table when rows exist, else the selector string.
        if !self.providers.is_empty() {
            // Rows emit in `providers` order, so the dump round-trips PRIORITY, not
            // just content (config §6 decision 1): row order IS routing priority, and
            // a dump that reordered rows would silently re-route the config it claims
            // to reproduce. Nothing sorts — the `[[provider]]` array's line order is
            // priority's one wire form, so `parse(dump(cfg))` preserves it by
            // construction (config §2.2).
            let rows: Vec<Row> = self
                .providers
                .iter()
                .map(|(name, inner)| Row { name, inner })
                .collect();
            m.serialize_entry("provider", &rows)?;
        } else if let Some(v) = &self.provider {
            m.serialize_entry("provider", v)?;
        }
        // The `[ingress]` table rides the dump like a row (ingress §6, config
        // §6) — its token already redacted by `redact()` above.
        if let Some(v) = &self.ingress {
            m.serialize_entry("ingress", v)?;
        }
        // The valve: an unmodeled passthrough key. A JSON null has no TOML form,
        // so it is dropped rather than failing the dump (config §6).
        for (key, value) in &self.extra {
            if !value.is_null() {
                m.serialize_entry(key, value)?;
            }
        }
        m.end()
    }
}

impl Serialize for PartialIngress {
    /// The dump encoding of the `[ingress]` table (config §6): present fields
    /// only, so the dump stays sparse and round-trips. Manual (not derived) so
    /// the `token` goes through `expose()` — the single audited plaintext read
    /// — rather than `Secret`'s credential-file `Serialize`; `dump_config`
    /// redacts it to the sentinel first.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if let Some(v) = &self.dialect {
            m.serialize_entry("dialect", v)?;
        }
        if let Some(v) = &self.listen {
            m.serialize_entry("listen", v)?;
        }
        if let Some(v) = &self.token {
            m.serialize_entry("token", v.expose())?;
        }
        if let Some(v) = &self.lossy {
            m.serialize_entry("lossy", v)?;
        }
        if !self.lossy_overrides.is_empty() {
            m.serialize_entry("lossy_overrides", &self.lossy_overrides)?;
        }
        m.end()
    }
}
