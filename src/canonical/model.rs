//! The canonical model list (§3, §4): one available model and the one pure
//! resolver that expands a seed against the provider's ORDERED list. No IO; the
//! list is the single source for "which model" — order is authoritative, there is
//! no separate rank field. Each `Protocol::decode_models` projects its dialect's
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

/// Resolve a `seed` against the provider's ordered model list (§4) — the SAME
/// operation for default-selection and partial-matching, distinguished only by
/// whether the seed is empty (the empty-input dissolve of a special case):
///   - `seed == ""` → the default: the first model flagged `default`, else
///     `models[0]`.
///   - `seed != ""` → an exact `id` if present, else the FIRST id (in list order)
///     whose `id` contains the seed case-insensitively ("opus" → "claude-opus-4-…").
///
/// List order is authoritative — the first match is "the suggested version" the
/// user asked for, never an ambiguity error (unlike provider routing, §7).
/// Exact-before-contains so a full id the row simply doesn't prefix-own resolves
/// to itself rather than to a longer id that merely contains it. An empty list or
/// an unmatched non-empty seed is `Config` (→78): the request cannot be routed to
/// a real model, the message naming the seed and a few available ids.
pub fn select_model(models: &[Model], seed: &str) -> Result<String, CanonicalError> {
    if seed.is_empty() {
        let chosen = models
            .iter()
            .find(|m| m.default)
            .or_else(|| models.first())
            .ok_or_else(|| no_model("no models available for default selection"))?;
        return Ok(chosen.id.clone());
    }
    let lower = seed.to_ascii_lowercase();
    models
        .iter()
        .find(|m| m.id == seed)
        .or_else(|| {
            models
                .iter()
                .find(|m| m.id.to_ascii_lowercase().contains(&lower))
        })
        .map(|m| m.id.clone())
        .ok_or_else(|| {
            no_model(&format!(
                "no model matches '{seed}'; available: {}",
                available(models)
            ))
        })
}

/// A model-resolution failure → `Config` (exit 78), the same family as
/// `NoProvider`/`AmbiguousModel` (§7): the request cannot reach a real model.
fn no_model(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// A few available ids for the unmatched-seed diagnostic — the first three in list
/// order, `…` when more follow, so the message is bounded yet orienting.
fn available(models: &[Model]) -> String {
    let mut shown: Vec<&str> = models.iter().take(3).map(|m| m.id.as_str()).collect();
    if models.len() > 3 {
        shown.push("…");
    }
    shown.join(", ")
}
