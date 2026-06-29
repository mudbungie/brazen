//! The canonical model list (§3, §4): one available model and the one pure, TOTAL
//! resolver that places a seed against the provider's ORDERED cached list. No IO;
//! the list is the single source for "which model" — order is authoritative, there
//! is no separate rank field. Each `Protocol::decode_models` projects its dialect's
//! list shape onto `Vec<Model>` (§3.1); `select_model` reads it.

use serde::{Deserialize, Serialize};

use crate::canonical::{CanonicalError, ErrorKind};

/// One available model, the canonical projection of a provider list entry (§3).
/// Ordered position in the returned `Vec` IS the provider's suggested order — the
/// single source the heuristics read, so there is no rank field. `default` is
/// CARRIED, not invented: a dialect that flags one sets it; today none does, so it
/// is `false` and §4's first-in-list rule governs — the seam stays so a provider
/// that DOES flag one needs no code change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub default: bool,
}

/// What produced the wire id — the provenance the §5.3 404 hint reads (CARRIED, not
/// reconstructed downstream: AGENTS.md). `Cached` is an entry resolved from the list
/// (an exact id, a partial match, or the default); `Verbatim` is the seed passed
/// through because the cache could not place it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    Cached,
    Verbatim,
}

/// Resolve a `seed` against the provider's cached model list (§4) — ONE TOTAL
/// operation for default-selection, partial-matching, and "the cache can't help, try
/// it literally," distinguished only by the seed and whether a match is found (the
/// empty-input dissolve of a special case, AGENTS.md):
///   - `seed == ""` → the default: the first model flagged `default`, else
///     `models[0]` (`Cached`). An EMPTY list here is the lone error — `Config` (78):
///     nothing to send and no list to default from. `provider` names it in that
///     message (carried, not reconstructed — the caller already knows it: AGENTS.md).
///   - `seed != ""` → an exact `id` if present (`Cached`); else the FIRST id (in list
///     order) whose `id` contains the seed case-insensitively (`Cached`, "opus" →
///     "claude-opus-4-…"); else the SEED ITSELF (`Verbatim`) — attempted literally,
///     since the cache cannot place it. A cold cache (empty list) therefore yields
///     `Verbatim` for any non-empty seed: cache-absent ≡ cache-present-but-empty.
///
/// List order is authoritative — the first match is "the suggested version," never an
/// ambiguity error. Exact-before-contains so a full id present in the list resolves to
/// itself rather than a longer id that merely contains it. Verbatim-on-no-match (not an
/// error) self-heals a stale cache: a brand-new full id no list yet carries is tried
/// verbatim and succeeds; a partial typo is tried verbatim, 404s, and the caller runs
/// `bz --list-models` (§5.3).
pub fn select_model(
    models: &[Model],
    seed: &str,
    provider: &str,
) -> Result<(String, Provenance), CanonicalError> {
    if seed.is_empty() {
        let chosen = models
            .iter()
            .find(|m| m.default)
            .or_else(|| models.first())
            .ok_or_else(|| no_default(provider))?;
        return Ok((chosen.id.clone(), Provenance::Cached));
    }
    let lower = seed.to_ascii_lowercase();
    let matched = models
        .iter()
        .find(|m| m.id == seed)
        .or_else(|| {
            models
                .iter()
                .find(|m| m.id.to_ascii_lowercase().contains(&lower))
        })
        .map(|m| m.id.clone());
    Ok(match matched {
        Some(id) => (id, Provenance::Cached),
        None => (seed.to_owned(), Provenance::Verbatim),
    })
}

/// The lone `select_model` failure (§4): `seed == "" && models.is_empty()` — no model
/// given and no cache to default from → `Config` (exit 78), the same family as
/// `NoProvider`/`AmbiguousModel` (config §7). The caller's next move is in the message,
/// which names `provider` so a multi-provider user knows which cache is cold.
fn no_default(provider: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: format!(
            "no model given and no model cache for {provider}; pass --model or run `bz --list-models`"
        ),
        provider_detail: None,
    }
}
