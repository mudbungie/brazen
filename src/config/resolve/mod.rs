//! Resolution: `into_resolved` validates, routes to one complete provider row,
//! and substitutes the model alias once (config §3, §7). The fold itself is just
//! `flags.or(env).or(file).or(defaults)` at the call site — precedence is the
//! *order of the operands*, data the reader can see, not control flow. The
//! request is NOT a fold operand: only its `model` is consulted, for routing
//! (arch §4.3, §4.4); everything it omits is filled later by `fill_absent`.
//! This module owns validation + routing; lifting the routed row into a complete
//! `Provider` (and the gen-scalar `body_defaults` take-offs) lives in [`row`].

use crate::canonical::{select_model, Provenance};
use crate::config::errors::ConfigError;
use crate::config::partial::{or_map, OutMode, PartialConfig, PartialProvider};
use crate::config::resolved::ResolvedConfig;
use crate::store::ModelCache;

mod ingress;
mod row;

pub(crate) use ingress::IngressConfig;

impl PartialConfig {
    /// Validate, route to a single complete provider row, and substitute the
    /// model alias once (config §7). Every failure is a `ConfigError` → 78.
    /// `cache` is the discovery seam routing's SECOND ownership tier reads (below);
    /// `None` is a path that carries no such seam (`--login`), for which it is inert —
    /// that path names its provider explicitly, so routing never asks.
    pub fn into_resolved(
        self,
        req_model: Option<&str>,
        cache: Option<&dyn ModelCache>,
    ) -> Result<ResolvedConfig, ConfigError> {
        self.check_scalars()?;
        // An EMPTY routing model is ABSENCE, not a seed that owns nothing: `cfg.model`
        // already spells "no model" as `""` (model-discovery §4), so `--model ""` and
        // `model = ""` resolve like the bare `bz "q"` — the first declared row, empty
        // seed — rather than falling to `NoProvider`. The empty-input dissolve, not a
        // case: both tiers below then only ever see a non-empty model to place.
        let routing_model = req_model
            .or(self.model.as_deref())
            .filter(|m| !m.is_empty());
        let (name, mut partial) = self.route(routing_model, cache)?;
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
    /// arch §4.3) — and the list is read once PER OWNERSHIP TIER: every row's
    /// CLAIMS first, then every row's CACHE. That split IS the precedence rule: a
    /// claim on any row beats a cache hit on any other, so a stale or 2xx-learned
    /// cache entry on an early row can never steal a family a later row claims
    /// outright. With NEITHER a name NOR a model (the zero-config `bz "q"`),
    /// the head of the same list is the default — config-file order, "whatever
    /// you find first reading from the top", so a user's rows beat the built-in
    /// defaults' rather than the alphabetically-first name. All of it is reads of
    /// one list, not an algorithm layered over it. `NoProvider` is what is left:
    /// a model zero rows own by EITHER tier, or an empty list with no head.
    fn route(
        &self,
        routing_model: Option<&str>,
        cache: Option<&dyn ModelCache>,
    ) -> Result<(String, PartialProvider), ConfigError> {
        if let Some(name) = &self.provider {
            let row = self
                .row(name)
                .ok_or_else(|| ConfigError::UnknownProvider { name: name.clone() })?;
            return Ok((name.clone(), row.clone()));
        }
        let hit = match routing_model {
            None => self.providers.first(),
            Some(model) => self
                .providers
                .iter()
                .find(|(_, row)| row_claims(row, model))
                .or_else(|| {
                    self.providers
                        .iter()
                        .find(|(name, _)| cache_places(cache, name, model))
                }),
        };
        hit.map(|(name, row)| (name.clone(), row.clone()))
            .ok_or(ConfigError::NoProvider)
    }
}

/// TIER 2 — a row also owns a model its OBSERVED list can place: the seed matches
/// an id in that provider's cached model list under the very `select_model`
/// semantics `generate` will apply to it (exact, else first case-insensitive
/// substring — model-discovery §4). `Cached` is exactly "the cache placed it";
/// `Verbatim` is "it could not", i.e. no ownership. This is what lets `bz --model
/// 5.5 "q"` reach an openai row past an anthropic one declared above it: `5.5`
/// substring-matches a cached `gpt-5.5` and does NOT match `claude-sonnet-5-5`.
/// It is a LOCAL read of the same file the request half reads, never a probe: a
/// row with no cache file simply cannot win this tier (arch §4.3), and it is
/// reached only when tier 1 found nothing, so the ordinary claim path pays
/// nothing. `cache: None` (a path with no discovery seam) is the cold cache.
fn cache_places(cache: Option<&dyn ModelCache>, provider: &str, model: &str) -> bool {
    let models = cache.and_then(|c| c.get(provider)).unwrap_or_default();
    matches!(
        select_model(&models, model, provider),
        Ok((_, Provenance::Cached))
    )
}

/// TIER 1 — a row CLAIMS the routing model when it spells it explicitly in
/// `model_aliases` (substitution shorthand) OR claims its family via a
/// `model_prefixes` entry (arch §4.3). Either is enough, and the rule never asks
/// WHICH kind of claim matched — only which row came first, `route`'s greedy
/// `find`. A claim is the operator's own declaration, which is why it outranks
/// every row's observed cache wholesale rather than row by row.
fn row_claims(row: &PartialProvider, model: &str) -> bool {
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
