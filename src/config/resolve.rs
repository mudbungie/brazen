//! Resolution: `into_resolved` validates, routes to one complete provider row,
//! and substitutes the model alias once (config §3, §7). The fold itself is just
//! `flags.or(env).or(file).or(defaults)` at the call site — precedence is the
//! *order of the operands*, data the reader can see, not control flow. The
//! request is NOT a fold operand: only its `model` is consulted, for routing
//! (arch §4.3, §4.4); everything it omits is filled later by [`fill_absent`].

use crate::config::errors::ConfigError;
use crate::config::partial::{OutMode, PartialConfig, PartialProvider};
use crate::config::provider::{AuthId, Provider};
use crate::config::resolved::ResolvedConfig;

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
