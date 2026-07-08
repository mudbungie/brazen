//! Resolution: `into_resolved` validates, routes to one complete provider row,
//! and substitutes the model alias once (config §3, §7). The fold itself is just
//! `flags.or(env).or(file).or(defaults)` at the call site — precedence is the
//! *order of the operands*, data the reader can see, not control flow. The
//! request is NOT a fold operand: only its `model` is consulted, for routing
//! (arch §4.3, §4.4); everything it omits is filled later by `fill_absent`.
//! This module owns validation + routing; lifting the routed row into a complete
//! `Provider` (and the gen-scalar `body_defaults` take-offs) lives in [`row`].

use crate::config::errors::ConfigError;
use crate::config::partial::{or_map, OutMode, PartialConfig, PartialProvider};
use crate::config::resolved::ResolvedConfig;

mod row;

impl PartialConfig {
    /// Validate, route to a single complete provider row, and substitute the
    /// model alias once (config §7). Every failure is a `ConfigError` → 78.
    pub fn into_resolved(self, req_model: Option<&str>) -> Result<ResolvedConfig, ConfigError> {
        self.check_scalars()?;
        let routing_model = req_model.or(self.model.as_deref());
        let (name, mut partial) = self.route(routing_model)?;
        // The top-level `base_url` scalar (flag>env>file) overrides the ROUTED row's
        // host, exactly as `--model` overrides the routing model: same provider,
        // different endpoint (config §4.5). `None` defers, leaving the row's own
        // `base_url`; a value replaces it BEFORE `row::complete` lifts the row — so
        // protocol/auth/api_header stay the row's, and this never creates a row.
        partial.base_url = self.base_url.or(partial.base_url);
        // The routed row's body_defaults (config §4.1): gen scalars fold into the
        // resolved typed fields beneath flag/env/file; whatever is LEFT is non-gen
        // passthrough, merged OVER the top-level `extra` (the row is more specific).
        // `take_*` removes each gen key, so `bd` is exactly the passthrough remainder.
        let mut bd = std::mem::take(&mut partial.body_defaults);
        let max_tokens = self.max_tokens.or(row::take_u32(&mut bd, "max_tokens")?);
        let temperature = self.temperature.or(row::take_f32(&mut bd, "temperature")?);
        let top_p = self.top_p.or(row::take_f32(&mut bd, "top_p")?);
        let stream = self.stream.or(row::take_bool(&mut bd, "stream")?);
        let extra = or_map(bd, self.extra);
        let provider = row::complete(name, partial)?;
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
            // The portable reasoning knob folds flag>env>file (the standard
            // `PartialConfig::or`); it is deliberately NOT taken from the row's
            // `body_defaults` — that map stays the raw-object escape hatch, riding
            // `extra` to the wire verbatim (config §4.1, providers.md §6).
            reasoning: self.reasoning,
            stream,
            timeout: self.timeout,
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
