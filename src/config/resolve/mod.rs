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

mod ingress;
mod row;

pub(crate) use ingress::IngressConfig;

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
        self.check_prefixes()
    }

    /// An EMPTY-STRING `model_prefixes` element is a `BadValue` (config §7) — the
    /// one validation greedy-first makes load-bearing: `"anything".starts_with("")`
    /// is true, so such a row owns EVERY model and, declared early, silently
    /// swallows all routing with no diagnostic. It is never an authored priority,
    /// only a typo or a half-deleted entry, so it is refused rather than obeyed —
    /// the general rule that a value which cannot be meant is not honored. The
    /// empty LIST (`model_prefixes = []`, claiming nothing) stays legal: it is how
    /// a row opts out of family routing (`openai-responses` ships it). Checked
    /// across ALL rows, not just the routed one — the offending row is exactly the
    /// one that would win, so a routed-row-only check could never fire.
    fn check_prefixes(&self) -> Result<(), ConfigError> {
        for (name, row) in &self.providers {
            let empty = row
                .model_prefixes
                .as_ref()
                .is_some_and(|ps| ps.iter().any(String::is_empty));
            if empty {
                return Err(bad(
                    "model_prefixes",
                    &format!("provider `{name}`: an empty prefix would own every model"),
                ));
            }
        }
        Ok(())
    }

    /// Resolve the single provider row, reading `providers` — the PRIORITY LIST
    /// (arch §4.3.1) — forward. An explicit name is a by-name scan and the one
    /// order-INSENSITIVE step: it overrides routing outright. Else the list is
    /// read greedily: with a routing model, the FIRST row that OWNS it wins
    /// (`find` stops — further owners are never consulted and are NOT an error;
    /// a total order always has a first element, so there is no tie to surface,
    /// arch §4.3); with NEITHER a name NOR a model (the zero-config `bz "q"`),
    /// the head of the same list is the default — config-file order, "whatever
    /// you find first reading from the top", so a user's rows beat the built-in
    /// defaults' rather than the alphabetically-first name. Both are one read of
    /// one list, not an algorithm layered over it. `NoProvider` is what is left:
    /// a model zero rows own, or an empty list with no head to fall back to.
    fn route(&self, routing_model: Option<&str>) -> Result<(String, PartialProvider), ConfigError> {
        if let Some(name) = &self.provider {
            let row = self
                .row(name)
                .ok_or_else(|| ConfigError::UnknownProvider { name: name.clone() })?;
            return Ok((name.clone(), row.clone()));
        }
        let hit = match routing_model {
            None => self.providers.first(),
            Some(model) => self.providers.iter().find(|(_, row)| row_owns(row, model)),
        };
        hit.map(|(name, row)| (name.clone(), row.clone()))
            .ok_or(ConfigError::NoProvider)
    }
}

/// A row OWNS the routing model when it spells it explicitly in `model_aliases`
/// (substitution shorthand) OR claims its family via a `model_prefixes` entry
/// (arch §4.3). Either is enough, and the rule never asks WHICH kind of claim
/// matched — only which row came first, which is `route`'s greedy `find`.
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
