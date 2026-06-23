//! The injected environment (config §3.4, arch §6.5). The library NEVER reads
//! `std::env`: `main` snapshots the process environment into an `EnvSnapshot`
//! once and injects it, so `partial_from_env` is a pure, table-driven mapping
//! and the whole env-precedence behavior is a table test. `config_path` is the
//! same `Option::or` shape one level up — it answers *which file*, never *which
//! value wins* (config §5).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::errors::ConfigError;
use crate::config::partial::{OutMode, PartialConfig};
use crate::store::Secret;

/// A snapshot of the process environment, injected by `main`. A newtype over a
/// `BTreeMap` so the projection is deterministic and pure (config §3.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvSnapshot(pub BTreeMap<String, String>);

impl EnvSnapshot {
    /// The value of an environment variable, if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// Build the env-layer `PartialConfig` from the injected snapshot (config §3.4).
/// `$BRAZEN_CONFIG` is deliberately absent — it selects *which file*, a
/// pre-resolve concern (config §5). A scalar that fails `from_str` is a
/// `BadValue` (config §7), so the projection is fallible but still pure.
pub fn partial_from_env(env: &EnvSnapshot) -> Result<PartialConfig, ConfigError> {
    let output = match env.get("BRAZEN_OUTPUT") {
        Some(v) => Some(OutMode::parse(v).ok_or_else(|| ConfigError::BadValue {
            key: "BRAZEN_OUTPUT".into(),
            detail: format!("unknown output mode `{v}`"),
        })?),
        None => None,
    };
    Ok(PartialConfig {
        provider: env.get("BRAZEN_PROVIDER").map(str::to_owned),
        model: env.get("BRAZEN_MODEL").map(str::to_owned),
        // BRAZEN_API_KEY outranks the vendor-conventional ANTHROPIC_API_KEY alias.
        api_key: env
            .get("BRAZEN_API_KEY")
            .or_else(|| env.get("ANTHROPIC_API_KEY"))
            .map(Secret::new),
        output,
        thinking: parse_scalar("BRAZEN_THINKING", env)?,
        max_tokens: parse_scalar("BRAZEN_MAX_TOKENS", env)?,
        temperature: parse_scalar("BRAZEN_TEMPERATURE", env)?,
        top_p: parse_scalar("BRAZEN_TOP_P", env)?,
        stream: parse_scalar("BRAZEN_STREAM", env)?,
        timeout_connect: parse_scalar("BRAZEN_TIMEOUT_CONNECT", env)?,
        timeout_response: parse_scalar("BRAZEN_TIMEOUT_RESPONSE", env)?,
        timeout_idle: parse_scalar("BRAZEN_TIMEOUT_IDLE", env)?,
        ..Default::default()
    })
}

/// Parse one optional env scalar, mapping a `from_str` failure to `BadValue`
/// (config §7); an absent variable is `None`.
fn parse_scalar<T: std::str::FromStr>(
    key: &str,
    env: &EnvSnapshot,
) -> Result<Option<T>, ConfigError> {
    match env.get(key) {
        Some(value) => value.parse().map(Some).map_err(|_| ConfigError::BadValue {
            key: key.to_owned(),
            detail: format!("could not parse `{value}`"),
        }),
        None => Ok(None),
    }
}

/// Locate the config file (config §5): `--config` > `$BRAZEN_CONFIG` > XDG. The
/// same `Option::or` shape as the value fold, one level up. The flag here only
/// changes *which file* the file layer reads — never the layer precedence.
pub fn config_path(explicit: Option<PathBuf>, env: &EnvSnapshot) -> PathBuf {
    explicit
        .or_else(|| env.get("BRAZEN_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| xdg_config_home(env).join("brazen/config.toml"))
}

/// `$XDG_CONFIG_HOME`, else `~/.config`, else a relative `.config` when even
/// `$HOME` is unset — all read from the injected snapshot, never the process.
fn xdg_config_home(env: &EnvSnapshot) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg);
    }
    match env.get("HOME") {
        Some(home) => PathBuf::from(home).join(".config"),
        None => PathBuf::from(".config"),
    }
}
