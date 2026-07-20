//! The canonical model list (§3, §4): one available model and the one pure, TOTAL
//! resolver that places a seed against the provider's ORDERED cached list. No IO;
//! the list is the single source for "which model" — order is authoritative, there
//! is no separate rank field. The one generic `decode_models`, fed each protocol's
//! `Protocol::models_shape` (§3.1), projects the dialect's list body onto `Vec<Model>`;
//! `select_model` reads it.

use serde::{Deserialize, Serialize};

use crate::canonical::{CanonicalError, ErrorKind};

/// One available model, the canonical projection of a provider list entry (§3).
/// Ordered position in the returned `Vec` IS the provider's suggested order — the
/// single source the heuristics read, so there is no rank field. `default` is
/// CARRIED, not invented: a dialect that flags one sets it; today none does, so it
/// is `false` and §4's first-in-list rule governs — the seam stays so a provider
/// that DOES flag one needs no code change.
///
/// The three metadata fields are the provider-reported facts a harness would else
/// hand-mirror (model-discovery §3): `context_window` (input token limit),
/// `max_output_tokens` (output limit), `display_name` (human label). Each is
/// OPTION-SHAPED and CARRIED, never fabricated — absent stays `None` (the Usage
/// zero-vs-unknown principle, AGENTS.md): Google serves all three on its list GET,
/// Anthropic only `display_name`, and OpenAI/Ollama none (so those rows stay `None`;
/// the empty-set rule — a harness hand-configures only what no provider serves). The
/// fields are ADDITIVE with `serde(default)` + `skip_serializing_if`: an older cache
/// (id + default only) reads clean to `None`, and a metadata-less model serializes
/// byte-identically to the pre-metadata `{id,default}` shape — the v=1 grows-only
/// discipline, so `--json`/the on-disk cache both extend without a version break.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// One provider's cache document (§5.1): the ORDERED list plus `last_used`, the local
/// observation of which id this provider last served a 2xx for. Two DIFFERENT facts in
/// one file, never two representations of one: `models` is MEMBERSHIP (which ids this
/// row can serve — read by partial matching and by routing's ownership tier, config §7),
/// `last_used` is RECENCY (which of them to default to on an empty seed, §4 rung 2). The
/// pointer is never a permutation of the list — §5.4's "append, never reorder" stands.
///
/// INVARIANT, held by the one write site (§5.4): after a 2xx, `last_used` names an id
/// that is in `models`. It is nevertheless read FORGIVINGLY — an absent, empty, or
/// dangling `last_used` simply falls through to the provider's own suggestion, never an
/// error (§5.1). ADDITIVE like the metadata keys: `serde(default)` +
/// `skip_serializing_if` means a pre-`last_used` cache reads clean to `None` and a
/// pointer-less document serializes byte-identically to the old `{"models":[…]}` shape,
/// so the grows-only v=1 discipline holds with no cache-version field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CachedModels {
    pub models: Vec<Model>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

impl CachedModels {
    /// The document a wholesale `--list-models` write produces: the discovered list,
    /// carrying `last_used` forward. Re-listing changes WHICH IDS EXIST, never WHICH ONE
    /// YOU LAST USED — a fact discovery has no opinion about. If the new list no longer
    /// contains it the pointer simply dangles, and §4 falls through (forgiving read).
    pub fn relist(&self, models: Vec<Model>) -> CachedModels {
        CachedModels {
            models,
            last_used: self.last_used.clone(),
        }
    }
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
///   - `seed == ""` → the default, taken down the §4 ladder: the id `last_used` names
///     if the list still carries it (rung 2 — YOUR observed choice), else the first
///     model flagged `default`, else `models[0]` (rung 3 — the PROVIDER's suggestion),
///     all `Cached`. Rung 2 outranks the flag and the head because config outranks all
///     else BECAUSE IT ENCODES INTENT — and a model you actually reached for is nearer
///     to intent than a list position nobody chose. An EMPTY list here is the lone
///     error — `Config` (78): nothing to send and no list to default from. `provider`
///     names it in that message (carried, not reconstructed — the caller already knows
///     it: AGENTS.md).
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
    cached: &CachedModels,
    seed: &str,
    provider: &str,
) -> Result<(String, Provenance), CanonicalError> {
    let models = &cached.models;
    if seed.is_empty() {
        let chosen = cached
            .last_used
            .iter()
            .find_map(|id| models.iter().find(|m| &m.id == id))
            .or_else(|| models.iter().find(|m| m.default))
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
/// `NoProvider` (config §7). The caller's next move is in the message,
/// which names `provider` so a multi-provider user knows which cache is cold.
fn no_default(provider: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: format!(
            "no model given and no model cache for {provider}; pass --model or run `bz --list-models`"
        ),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
