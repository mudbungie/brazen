//! Routing's SECOND ownership tier (arch §4.3, config §7 step 3b): a row also
//! owns a model its OBSERVED (cached) list can place, so a seed no row CLAIMS
//! falls THROUGH the rows that cannot serve it to the one that can. The tiers are
//! walked whole — every row by claim, then every row by cache — which is the
//! precedence ruling under test here: a cache hit never outranks a claim, on any
//! row, in any order. `config_priority` owns the within-tier greedy rule.

use crate::testing::MemoryModelCache;
use crate::tests::config_support::{file, no_env, req, resolve_cached};

use crate::{ConfigError, Model, ModelCache, PartialConfig};

/// A complete row named `name` claiming `prefix` as its family.
fn row(name: &str, prefix: &str) -> String {
    format!(
        "[[provider]]\nname = \"{name}\"\nbase_url = \"https://{name}.test\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nmodel_prefixes = [\"{prefix}\"]\n"
    )
}

fn listed(ids: &[&str]) -> Vec<Model> {
    ids.iter()
        .map(|id| Model {
            id: (*id).to_owned(),
            ..Default::default()
        })
        .collect()
}

/// The user's config: `anthropic` declared ABOVE `openai`, each claiming its own
/// family, each with a cache a prior `bz --list-models` wrote.
fn two_rows() -> String {
    format!("{}{}", row("anthropic", "claude-"), row("openai", "gpt-"))
}

fn primed() -> MemoryModelCache {
    MemoryModelCache::with(
        "anthropic",
        listed(&["claude-sonnet-5-5", "claude-opus-4-1"]),
    )
    .and("openai", listed(&["gpt-5.5", "gpt-4o"]))
}

/// Route `model` against `rows` with no `--provider`, with `cache` armed.
fn route(rows: &str, model: &str, cache: Option<&dyn ModelCache>) -> Result<String, ConfigError> {
    resolve_cached(
        PartialConfig::default(),
        &no_env(),
        file(rows),
        PartialConfig::default(),
        Some(&req(model)),
        cache,
    )
    .map(|cfg| cfg.provider.name)
}

#[test]
fn a_seed_no_row_claims_falls_through_to_the_row_whose_cache_places_it() {
    // THE case: `bz --model 5.5 "q"` with anthropic first. No row CLAIMS `5.5` —
    // it is neither an alias nor a prefix of any family — so tier 2 asks each
    // row's cache under `select_model`'s own semantics (model-discovery §4).
    // Anthropic's list cannot place it ("5.5" with a DOT is not a substring of
    // "claude-sonnet-5-5"); openai's can ("gpt-5.5"). Without the second tier
    // this is `NoProvider`/78, which is the bug the user hit.
    let cache = primed();
    assert_eq!(route(&two_rows(), "5.5", Some(&cache)).unwrap(), "openai");
    // The seed is carried VERBATIM into the routed row — routing places it to pick
    // the row, `generate` places it again against the same cache to get the wire id.
    // Ownership is a query, not a substitution (arch §4.3).
    let cfg = resolve_cached(
        PartialConfig::default(),
        &no_env(),
        file(&two_rows()),
        PartialConfig::default(),
        Some(&req("5.5")),
        Some(&cache),
    )
    .unwrap();
    assert_eq!(cfg.model, "5.5");
}

#[test]
fn claims_are_unregressed_and_never_consult_the_cache() {
    // The first tier is untouched: a claimed family routes by the claim alone,
    // identically with the cache armed and with no cache seam at all.
    let cache = primed();
    for (model, provider) in [
        ("claude-opus-4-1", "anthropic"),
        ("gpt-5.5", "openai"),
        ("gpt-4o", "openai"),
    ] {
        assert_eq!(route(&two_rows(), model, Some(&cache)).unwrap(), provider);
        assert_eq!(route(&two_rows(), model, None).unwrap(), provider);
    }
}

#[test]
fn a_claim_on_a_later_row_beats_a_cache_hit_on_an_earlier_one() {
    // THE PRECEDENCE RULING. `anthropic` is declared first AND its cache carries
    // `gpt-5.5` — the shape learn-on-success (model-discovery §5.4) can mint, since
    // one explicit `--provider anthropic --model gpt-5.5` run that returned 2xx
    // appends that id to anthropic's list. Under a row-at-a-time walk that stale
    // entry would silently hijack the whole `gpt-` family from the row that CLAIMS
    // it, permanently and invisibly. Walking the tiers whole makes it impossible:
    // the claim is checked on EVERY row before any cache is read.
    let poisoned = MemoryModelCache::with("anthropic", listed(&["claude-sonnet-5-5", "gpt-5.5"]))
        .and("openai", listed(&["gpt-5.5"]));
    assert_eq!(
        route(&two_rows(), "gpt-5.5", Some(&poisoned)).unwrap(),
        "openai"
    );
    // A learned id no row claims IS still reachable — the tier is a fall-through,
    // not a quarantine: `sonnet-5` matches only anthropic's list.
    assert_eq!(
        route(&two_rows(), "sonnet-5", Some(&poisoned)).unwrap(),
        "anthropic"
    );
}

#[test]
fn within_the_cache_tier_the_first_declared_row_still_wins() {
    // Tier 2 is the SAME greedy read of the SAME priority list (arch §4.3.1): two
    // rows whose caches both place the seed is not an error and needs no tiebreak.
    // Reordered, the answer flips — which is what proves the order decided it.
    let both = MemoryModelCache::with("anthropic", listed(&["claude-5-5-turbo"]))
        .and("openai", listed(&["gpt-5-5-turbo"]));
    let flipped = format!("{}{}", row("openai", "gpt-"), row("anthropic", "claude-"));
    assert_eq!(
        route(&two_rows(), "5-5-t", Some(&both)).unwrap(),
        "anthropic"
    );
    assert_eq!(route(&flipped, "5-5-t", Some(&both)).unwrap(), "openai");
}

#[test]
fn the_match_is_select_models_own_case_insensitive_containment() {
    // No second matching rule was written: routing asks `select_model` (§4), so an
    // uppercase seed and an exact full id place exactly as they do at generation.
    let cache = primed();
    assert_eq!(
        route(&two_rows(), "OPUS", Some(&cache)).unwrap(),
        "anthropic"
    );
    assert_eq!(route(&two_rows(), "4O", Some(&cache)).unwrap(), "openai");
}

#[test]
fn a_cold_cache_simply_cannot_win_the_tier() {
    // NO PROBING (the ruling): routing never reaches the network, so a provider
    // brazen has never listed owns nothing beyond its claims and the seed is
    // `NoProvider` (78) exactly as before. `bz --list-models` is what arms the
    // tier — the documented cost of keeping routing a pure local read.
    let cold = MemoryModelCache::new();
    assert_eq!(
        route(&two_rows(), "5.5", Some(&cold)).unwrap_err(),
        ConfigError::NoProvider
    );
    // Same for a path carrying NO discovery seam at all (`--login`): cache-absent
    // is cache-present-but-empty, the identity element, not a special case.
    assert_eq!(
        route(&two_rows(), "5.5", None).unwrap_err(),
        ConfigError::NoProvider
    );
    // And a seed NO list can place is still `NoProvider` with the tier armed.
    assert_eq!(
        route(&two_rows(), "mistral-large", Some(&primed())).unwrap_err(),
        ConfigError::NoProvider
    );
}

#[test]
fn an_empty_routing_model_is_absence_not_a_seed_that_owns_nothing() {
    // `--model ""` / `model = ""` resolves like the bare `bz "q"`: the first
    // declared row with an empty seed, which `select_model` then reads as "the
    // default". Routing never asks either tier to place an empty string — an
    // empty seed is not a match question, and both tiers would answer nonsense
    // (every alias table misses it; every non-empty cache "defaults" to its head).
    let cfg = resolve_cached(
        PartialConfig::default(),
        &no_env(),
        file(&format!("model = \"\"\n{}", two_rows())),
        PartialConfig::default(),
        None,
        Some(&primed()),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.model, "");
}
