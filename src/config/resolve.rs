//! Resolution: `into_resolved` validates, routes to one complete provider row,
//! and substitutes the model alias once (config §3, §7). The fold itself is just
//! `flags.or(env).or(file).or(defaults)` at the call site — precedence is the
//! *order of the operands*, data the reader can see, not control flow. The
//! request is NOT a fold operand: only its `model` is consulted, for routing
//! (arch §4.3, §4.4); everything it omits is filled later by `fill_absent`.

use serde_json::{Map, Value};

use crate::config::errors::ConfigError;
use crate::config::partial::{or_map, OutMode, PartialConfig, PartialProvider};
use crate::config::provider::{AuthId, Provider};
use crate::config::resolved::ResolvedConfig;

impl PartialConfig {
    /// Validate, route to a single complete provider row, and substitute the
    /// model alias once (config §7). Every failure is a `ConfigError` → 78.
    pub fn into_resolved(self, req_model: Option<&str>) -> Result<ResolvedConfig, ConfigError> {
        self.check_scalars()?;
        let routing_model = req_model.or(self.model.as_deref());
        let (name, mut partial) = self.route(routing_model)?;
        // The routed row's body_defaults (config §4.1): gen scalars fold into the
        // resolved typed fields beneath flag/env/file; whatever is LEFT is non-gen
        // passthrough, merged OVER the top-level `extra` (the row is more specific).
        // `take_*` removes each gen key, so `bd` is exactly the passthrough remainder.
        let mut bd = std::mem::take(&mut partial.body_defaults);
        let max_tokens = self.max_tokens.or(take_u32(&mut bd, "max_tokens")?);
        let temperature = self.temperature.or(take_f32(&mut bd, "temperature")?);
        let top_p = self.top_p.or(take_f32(&mut bd, "top_p")?);
        let stream = self.stream.or(take_bool(&mut bd, "stream")?);
        let extra = or_map(bd, self.extra);
        let provider = complete(name, partial)?;
        // Resolution does routing + alias substitution only — never a model-cache
        // lookup (model-discovery §5: the probe is dissolved, no `needs_probe`). Alias
        // substitution is identity-passthrough: an unaliased model passes through
        // verbatim, so it never fails (arch §4.3). The result is a SEED — a full wire
        // id, a partial, or `""` (absent) — that `serve` places against the cache via
        // `select_model`. `model_prefixes` survives, but for ROUTING ONLY (`route`).
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
            // The cache lookup runs in `serve`, not here; the carried provenance is
            // `false` until then (and on `--raw`, which never reads the model).
            model_from_cache: false,
            output: self.output.unwrap_or(OutMode::Text),
            thinking: self.thinking.unwrap_or(false),
            inline_key: self.api_key,
            max_tokens,
            temperature,
            top_p,
            stream,
            timeout_connect: self.timeout_connect,
            timeout_response: self.timeout_response,
            timeout_idle: self.timeout_idle,
            system: self.system,
            extra,
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
    /// the row(s) that OWN the routing model — its `model_aliases` spell it, or a
    /// `model_prefixes` entry claims its family. Zero → none, two-or-more →
    /// ambiguity surfaced (arch §4.3). With NEITHER a name NOR a routing model
    /// (the zero-config `bz "q"`), default to the FIRST-DECLARED provider — the
    /// `default_provider` captured in config-file order (config §4.3), so a user's
    /// first row beats the built-in defaults rather than the alphabetically-first
    /// name. The empty-input dissolve of `NoProvider`, symmetric with
    /// `select_model`'s empty-seed → first cached model; a config with no provider
    /// at all (no `default_provider`) is the lone residue of `NoProvider` here.
    fn route(&self, routing_model: Option<&str>) -> Result<(String, PartialProvider), ConfigError> {
        if let Some(name) = &self.provider {
            let row = self
                .providers
                .get(name)
                .ok_or_else(|| ConfigError::UnknownProvider { name: name.clone() })?;
            return Ok((name.clone(), row.clone()));
        }
        let Some(model) = routing_model else {
            return self
                .default_provider
                .as_deref()
                .and_then(|name| self.providers.get_key_value(name))
                .map(|(name, row)| (name.clone(), row.clone()))
                .ok_or(ConfigError::NoProvider);
        };
        let mut matches: Vec<(String, PartialProvider)> = self
            .providers
            .iter()
            .filter(|(_, row)| row_owns(row, model))
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

/// A row OWNS the routing model when it spells it explicitly in `model_aliases`
/// (substitution shorthand) OR claims its family via a `model_prefixes` entry
/// (arch §4.3). Either is enough; both feed the one single-match routing query,
/// so two owning rows are still an `AmbiguousModel` (78), never a silent pick.
fn row_owns(row: &PartialProvider, model: &str) -> bool {
    let aliased = row
        .model_aliases
        .as_ref()
        .is_some_and(|a| a.contains_key(model));
    let prefixed = row
        .model_prefixes
        .as_ref()
        .is_some_and(|ps| ps.iter().any(|p| model.starts_with(p.as_str())));
    aliased || prefixed
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
    // A keyed row MUST carry an `api_header`; an `auth = "none"` row carries none.
    // Pairing it here makes a keyed impl's `api_header.is_some()` an invariant, never
    // a runtime branch — the same discipline as `oauth` below.
    let api_header = row.api_header;
    if auth != AuthId::None && api_header.is_none() {
        return Err(need("api_header"));
    }
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
        unsupported_body_keys: row.unsupported_body_keys.unwrap_or_default(),
        // The discovery override carries verbatim (config §4.4): nothing to fold into a
        // typed scalar, unlike body_defaults — the verb overlays it per key.
        models: row.models,
        oauth: row.oauth,
        ambient: row.ambient,
        name,
    })
}

/// Take a gen-scalar `u32` body default off the row (config §4.1): `None` if the
/// key is absent, the value if it is a positive integer in range, else `BadValue`
/// (→78). Removing the key leaves `bd` holding only non-gen passthrough.
fn take_u32(bd: &mut Map<String, Value>, key: &str) -> Result<Option<u32>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0 && *n <= u64::from(u32::MAX))
            .map(|n| Some(n as u32))
            .ok_or_else(|| bad(key, "must be a positive integer")),
    }
}

/// Take a gen-scalar `f32` body default off the row (config §4.1): `None` if
/// absent, the value if it is a number (an integer coerces), else `BadValue`.
fn take_f32(bd: &mut Map<String, Value>, key: &str) -> Result<Option<f32>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| bad(key, "must be a number")),
    }
}

/// Take a gen-scalar `bool` body default off the row (config §4.1): `None` if
/// absent, the value if it is a boolean, else `BadValue`.
fn take_bool(bd: &mut Map<String, Value>, key: &str) -> Result<Option<bool>, ConfigError> {
    match bd.remove(key) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| bad(key, "must be a boolean")),
    }
}
