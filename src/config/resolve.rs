//! Resolution: the fold under `Option::or` then `into_resolved` (config §3, §7).
//! The whole of resolution is one expression — precedence is the *order of the
//! operands*, data the reader can see, not control flow. The embedded defaults
//! travel the identical `toml::from_str` path as a user file (config §3.5); a
//! missing file is `PartialConfig::default()`, the identity of the fold (§3.3).

use std::path::Path;

use serde_json::{Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest, Content};
use crate::config::env::partial_from_env;
use crate::config::errors::ConfigError;
use crate::config::partial::{OutMode, PartialConfig, PartialProvider};
use crate::config::provider::{AuthId, Provider};
use crate::config::EnvSnapshot;
use crate::store::Secret;

/// The embedded provider table (arch §4.2), parsed through the SAME path as a
/// user file — "lowest precedence" is just "last operand," no bootstrap case.
const DEFAULTS_TOML: &str = include_str!("../../data/defaults.toml");

/// Parse a config string into a `PartialConfig`, mapping any TOML/serde failure
/// (typo'd key, duplicate provider name, bad syntax) to `MalformedFile` — the
/// one place a present-but-broken file becomes an error, distinct from a missing
/// file's identity element (config §3.3, §7).
pub fn parse_config(toml_str: &str) -> Result<PartialConfig, ConfigError> {
    toml::from_str(toml_str).map_err(|e| ConfigError::MalformedFile {
        detail: e.to_string(),
    })
}

/// Read the config file at `path` into a `PartialConfig` (config §3.3): a present
/// file parses (malformed → 78), a missing/unreadable one is the fold identity
/// `default()`. The one sanctioned config-file read in the lib — `run` and `bz
/// login` both go through it, so the path-resolution and malformed handling have
/// one home. The path came from the injected env, so it is tempfile-testable.
pub fn read_config_file(path: &Path) -> Result<PartialConfig, CanonicalError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config(&text).map_err(CanonicalError::from),
        Err(_) => Ok(PartialConfig::default()),
    }
}

/// The embedded defaults as a `PartialConfig`. The one sanctioned `expect`: it
/// is on our own compile-time-committed constant, validated by a unit test, not
/// on external input (config §3.5).
pub fn defaults() -> PartialConfig {
    #[allow(clippy::expect_used)]
    parse_config(DEFAULTS_TOML).expect("embedded defaults.toml is a valid, committed constant")
}

/// Resolve the four sparse inputs into one `ResolvedConfig` (config §3). The
/// request is NOT a fold operand — only its `model` is consulted, and only for
/// routing (arch §4.3, §4.4); everything the request omits is filled later by
/// [`fill_absent`].
pub fn resolve(
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
    defaults: PartialConfig,
    req: Option<&CanonicalRequest>,
) -> Result<ResolvedConfig, ConfigError> {
    let env = partial_from_env(env)?;
    let cfg = flags.or(env).or(file).or(defaults);
    cfg.into_resolved(req_model(req))
}

/// The request's `model` wins for routing when set; an empty string is "absent"
/// so config supplies it (arch §4.3).
fn req_model(req: Option<&CanonicalRequest>) -> Option<&str> {
    req.map(|r| r.model.as_str()).filter(|m| !m.is_empty())
}

impl PartialConfig {
    /// Validate, route to a single complete provider row, and substitute the
    /// model alias once (config §7). Every failure is a `ConfigError` → 78.
    pub fn into_resolved(self, req_model: Option<&str>) -> Result<ResolvedConfig, ConfigError> {
        self.check_scalars()?;
        let routing_model = req_model.or(self.model.as_deref());
        let (name, partial) = self.route(routing_model)?;
        let provider = complete(name, partial)?;
        // Alias substitution is identity-passthrough: an unaliased model passes
        // through verbatim, so substitution never fails (arch §4.3).
        let model = match routing_model {
            Some(m) => provider
                .model_aliases
                .get(m)
                .cloned()
                .unwrap_or_else(|| m.to_owned()),
            None => String::new(),
        };
        Ok(ResolvedConfig {
            provider,
            model,
            output: self.output.unwrap_or(OutMode::Text),
            thinking: self.thinking.unwrap_or(false),
            inline_key: self.api_key,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            stream: self.stream,
            system: self.system,
            extra: self.extra,
        })
    }

    /// A value that parses but is contradictory is a `BadValue` (config §7).
    fn check_scalars(&self) -> Result<(), ConfigError> {
        if self.max_tokens == Some(0) {
            return Err(bad("max_tokens", "must be greater than zero"));
        }
        if self.temperature.is_some_and(f32::is_nan) {
            return Err(bad("temperature", "must be a number"));
        }
        if self.top_p.is_some_and(f32::is_nan) {
            return Err(bad("top_p", "must be a number"));
        }
        Ok(())
    }

    /// Resolve the single provider row: an explicit name is a keyed lookup; else
    /// the row(s) whose `model_aliases` contain the routing model. Zero → none,
    /// two-or-more → ambiguity surfaced (arch §4.3).
    fn route(&self, routing_model: Option<&str>) -> Result<(String, PartialProvider), ConfigError> {
        if let Some(name) = &self.provider {
            let row = self
                .providers
                .get(name)
                .ok_or_else(|| ConfigError::UnknownProvider { name: name.clone() })?;
            return Ok((name.clone(), row.clone()));
        }
        let model = routing_model.ok_or(ConfigError::NoProvider)?;
        let mut matches: Vec<(String, PartialProvider)> = self
            .providers
            .iter()
            .filter(|(_, row)| aliases_contain(row, model))
            .map(|(name, row)| (name.clone(), row.clone()))
            .collect();
        if matches.is_empty() {
            return Err(ConfigError::NoProvider);
        }
        if matches.len() > 1 {
            return Err(ConfigError::AmbiguousModel {
                model: model.to_owned(),
                providers: matches.into_iter().map(|(name, _)| name).collect(),
            });
        }
        Ok(matches.swap_remove(0))
    }
}

fn aliases_contain(row: &PartialProvider, model: &str) -> bool {
    row.model_aliases
        .as_ref()
        .is_some_and(|a| a.contains_key(model))
}

fn bad(key: &str, detail: &str) -> ConfigError {
    ConfigError::BadValue {
        key: key.to_owned(),
        detail: detail.to_owned(),
    }
}

/// Lift a sparse, post-fold row into a complete `Provider`, surfacing each
/// missing required field by name (config §7 `IncompleteProvider`).
fn complete(name: String, row: PartialProvider) -> Result<Provider, ConfigError> {
    let need = |field| ConfigError::IncompleteProvider {
        name: name.clone(),
        field,
    };
    let base_url = row.base_url.ok_or_else(|| need("base_url"))?;
    let protocol = row.protocol.ok_or_else(|| need("protocol"))?;
    let auth = row.auth.ok_or_else(|| need("auth"))?;
    let api_header = row.api_header.ok_or_else(|| need("api_header"))?;
    // An `oauth2` row MUST carry an `oauth` block — resolution pairs the two or
    // fails here (auth §1.3), so the `OAuth2` impl's `oauth.is_some()` is an
    // invariant, never a runtime branch.
    if auth == AuthId::OAuth2 && row.oauth.is_none() {
        return Err(need("oauth"));
    }
    Ok(Provider {
        base_url,
        protocol,
        auth,
        api_header,
        beta_headers: row.beta_headers.unwrap_or_default(),
        model_aliases: row.model_aliases.unwrap_or_default(),
        default_max_tokens: row.default_max_tokens,
        oauth: row.oauth,
        name,
    })
}

/// The one config the pipeline runs on (config §7). `model` is the alias-
/// resolved WIRE id, so `ProviderCtx.model` is final and `encode` has no model
/// logic (arch §4.1). `raw` and `effective_max_tokens` are queries, not fields.
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
    /// The resolved leading system prompt (config §4, §7): `fill_absent` supplies
    /// it to a request that omits its own `system`. `None` is the no-system path.
    pub system: Option<Vec<Content>>,
    pub extra: Map<String, Value>,
}

impl ResolvedConfig {
    /// Is this a `--raw` run? A query over the output mode, one home for the
    /// fact (config §7).
    pub fn raw(&self) -> bool {
        self.output == OutMode::Raw
    }

    /// `max_tokens` after the row default: the resolved config value, else the
    /// provider row's `default_max_tokens` at lowest precedence (config §4.1).
    pub fn effective_max_tokens(&self) -> Option<u32> {
        self.max_tokens.or(self.provider.default_max_tokens)
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
    req.system = req.system.take().or_else(|| cfg.system.clone());
}
