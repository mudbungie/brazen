//! The §4 empty-seed LADDER and the §5.1 cache document — the other half of
//! `select_model`, split from the matching rules in `canonical_model`. Rung 1
//! (an explicit seed) never reaches here; this file pins rung 2 (`last_used`, the
//! model you actually reached for) beating rung 3 (the provider's own suggestion:
//! its `default` flag, else its list head), the forgiving fall-through when the
//! pointer dangles, and the additive `CachedModels` serde. Pure; no network.

use crate::{select_model, CachedModels, ErrorKind, Model, Provenance};
use serde_json::json;

fn model(id: &str, default: bool) -> Model {
    Model {
        id: id.into(),
        default,
        ..Default::default()
    }
}

/// A pointer-less document — the pre-`last_used` shape, i.e. rung 2 empty.
fn cached(models: &[Model]) -> CachedModels {
    CachedModels {
        models: models.to_vec(),
        last_used: None,
    }
}

/// The same document with the §5.4 pointer set — rung 2 present.
fn used(models: &[Model], last: &str) -> CachedModels {
    CachedModels {
        last_used: Some(last.into()),
        ..cached(models)
    }
}

#[test]
fn empty_seed_picks_the_flagged_default_else_first_in_list_cached() {
    // No flag: the FIRST in list order is the default (§4 first-in-list rule), Cached.
    let list = [model("a", false), model("b", false)];
    assert_eq!(
        select_model(&cached(&list), "", "anthropic").unwrap(),
        ("a".to_string(), Provenance::Cached)
    );

    // A later model flagged `default` wins over list order (carried, not invented).
    let flagged = [model("a", false), model("b", true), model("c", false)];
    assert_eq!(
        select_model(&cached(&flagged), "", "anthropic").unwrap(),
        ("b".to_string(), Provenance::Cached)
    );
}

#[test]
fn empty_seed_prefers_last_used_over_the_flag_and_the_head() {
    // §4 rung 2: the model you last used beats BOTH the provider's `default` flag and
    // its first-listed head — those are the provider's suggestion (rung 3), and an id
    // you actually reached for is nearer to intent. Cached, and the LIST IS UNTOUCHED:
    // the pointer is a pointer, never a permutation (§5.4 "append, never reorder").
    let list = [model("a", false), model("b", true), model("c", false)];
    assert_eq!(
        select_model(&used(&list, "c"), "", "anthropic").unwrap(),
        ("c".to_string(), Provenance::Cached)
    );
}

#[test]
fn last_used_never_overrides_an_explicit_seed() {
    // Rung 1 is absolute: an explicit --model/BRAZEN_MODEL/configured model is consulted
    // FIRST and rung 2 never runs. The pointer is read ONLY on an empty seed.
    let list = [
        model("claude-opus-4-1", false),
        model("claude-haiku-4", false),
    ];
    assert_eq!(
        select_model(&used(&list, "claude-haiku-4"), "opus", "anthropic")
            .unwrap()
            .0,
        "claude-opus-4-1"
    );
}

#[test]
fn a_dangling_or_empty_last_used_degrades_to_the_provider_suggestion() {
    // FORGIVING (§5.1): a pointer naming an id the list no longer carries — a
    // `--list-models` that dropped a deprecated model — falls through to rung 3 rather
    // than erroring or resurrecting the id. An empty pointer is the same fall-through
    // (it names no id), so no special case is written for it.
    let list = [model("a", false), model("b", false)];
    assert_eq!(
        select_model(&used(&list, "gone"), "", "openai").unwrap().0,
        "a"
    );
    assert_eq!(select_model(&used(&list, ""), "", "openai").unwrap().0, "a");
    // And a pointer over an EMPTY list is still the lone Config (78): the pointer must
    // resolve INTO the list, so it can never conjure a model out of nothing.
    assert_eq!(
        select_model(&used(&[], "a"), "", "openai")
            .unwrap_err()
            .kind,
        ErrorKind::Config
    );
}

#[test]
fn the_cache_document_is_additive_and_roundtrips() {
    // The grows-only v=1 discipline (§5.1): a pointer-less document serializes
    // byte-identically to the pre-`last_used` `{"models":[…]}` shape, and an old cache
    // reads clean to `last_used: None` — no cache-version field, no break.
    let doc = cached(&[model("a", false)]);
    assert_eq!(
        serde_json::to_value(&doc).unwrap(),
        json!({"models": [{"id": "a", "default": false}]})
    );
    let old: CachedModels =
        serde_json::from_value(json!({"models": [{"id": "a", "default": false}]})).unwrap();
    assert_eq!(old, doc);
    let with = used(&[model("a", false)], "a");
    let back: CachedModels = serde_json::from_value(serde_json::to_value(&with).unwrap()).unwrap();
    assert_eq!(back, with);
    assert!(!format!("{with:?}").is_empty());
    // `relist` REPLACES the list and CARRIES the pointer: discovery has no opinion about
    // which model you last used.
    let relisted = with.relist(vec![model("z", false)]);
    assert_eq!(relisted, used(&[model("z", false)], "a"));
}
