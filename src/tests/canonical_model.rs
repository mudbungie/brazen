//! Canonical model tests (§3, §4): the `Model` serde projection and the pure, TOTAL
//! `select_model` resolver — empty-seed default, exact-before-contains partial
//! matching, list order as the authoritative tiebreak, the `Verbatim` pass-through on
//! no match (self-healing a stale cache), and the lone `Config` (78) corner. Pure,
//! from literal inputs — no network.

use crate::{select_model, ErrorKind, Model, Provenance};
use serde_json::json;

fn model(id: &str, default: bool) -> Model {
    Model {
        id: id.into(),
        default,
        ..Default::default()
    }
}

#[test]
fn model_serializes_serde_direct_and_roundtrips() {
    // A metadata-less model serializes to EXACTLY `{id,default}` — the metadata Options
    // are `skip_serializing_if`'d away, so the on-disk cache/`--json` bytes are
    // byte-identical to the pre-metadata shape (the grows-only discipline, §3).
    let m = model("claude-opus-4-20250514", false);
    assert_eq!(
        serde_json::to_value(&m).unwrap(),
        json!({"id": "claude-opus-4-20250514", "default": false})
    );
    let back: Model = serde_json::from_value(json!({"id": "m", "default": true})).unwrap();
    assert_eq!(back, model("m", true));
    assert!(!format!("{m:?}").is_empty());
    assert_eq!(m.clone(), m);
}

#[test]
fn an_older_cache_entry_reads_clean_to_none_metadata() {
    // OLD-CACHE COMPAT (§3, §5.1): a cache/list entry written by a bz that predates the
    // metadata — `{id,default}` only — must still deserialize, the absent fields folding
    // to `None` (`serde(default)`), never a "missing field" error. This is the v=1
    // grows-only proof: an older writer's bytes read clean on the newer reader.
    let old: Model = serde_json::from_value(json!({"id": "gpt-5", "default": false})).unwrap();
    assert_eq!(old.context_window, None);
    assert_eq!(old.max_output_tokens, None);
    assert_eq!(old.display_name, None);
    assert_eq!(old, model("gpt-5", false));
}

#[test]
fn a_metadata_bearing_model_serializes_the_extra_fields_and_roundtrips() {
    // The additive fields ride the SAME serde (§3): a model with all three metadata facts
    // serializes them and round-trips byte-for-byte through the cache/`--json` shape.
    let m = Model {
        id: "gemini-2.5-pro".into(),
        default: false,
        context_window: Some(1_048_576),
        max_output_tokens: Some(65_536),
        display_name: Some("Gemini 2.5 Pro".into()),
    };
    assert_eq!(
        serde_json::to_value(&m).unwrap(),
        json!({
            "id": "gemini-2.5-pro", "default": false,
            "context_window": 1_048_576, "max_output_tokens": 65_536,
            "display_name": "Gemini 2.5 Pro"
        })
    );
    let back: Model = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(back, m);
}

#[test]
fn empty_seed_picks_the_flagged_default_else_first_in_list_cached() {
    // No flag: the FIRST in list order is the default (§4 first-in-list rule), Cached.
    let list = [model("a", false), model("b", false)];
    assert_eq!(
        select_model(&list, "", "anthropic").unwrap(),
        ("a".to_string(), Provenance::Cached)
    );

    // A later model flagged `default` wins over list order (carried, not invented).
    let flagged = [model("a", false), model("b", true), model("c", false)];
    assert_eq!(
        select_model(&flagged, "", "anthropic").unwrap(),
        ("b".to_string(), Provenance::Cached)
    );
}

#[test]
fn nonempty_seed_is_exact_before_contains_first_in_order_cached() {
    let list = [
        model("claude-opus-4-1", false),
        model("claude-opus-4-0", false),
        model("claude-sonnet-4-5", false),
    ];
    // Exact id wins even though it also "contains" itself — Cached.
    assert_eq!(
        select_model(&list, "claude-opus-4-0", "anthropic").unwrap(),
        ("claude-opus-4-0".to_string(), Provenance::Cached)
    );
    // Partial: the FIRST (list order) whose id contains the seed — "the suggested
    // version" — even though two ids contain "opus". Cached.
    assert_eq!(
        select_model(&list, "opus", "anthropic").unwrap(),
        ("claude-opus-4-1".to_string(), Provenance::Cached)
    );
    // Case-insensitive contains.
    assert_eq!(
        select_model(&list, "OPUS", "anthropic").unwrap().0,
        "claude-opus-4-1"
    );
    assert_eq!(
        select_model(&list, "SONNET", "anthropic").unwrap().0,
        "claude-sonnet-4-5"
    );
}

#[test]
fn exact_match_resolves_a_full_id_to_itself_cached() {
    // A full id present in the list resolves to itself (exact-before-contains), Cached,
    // rather than to a longer id that merely contains it.
    let list = [model("o1-pro", false), model("o1-pro-2024", false)];
    assert_eq!(
        select_model(&list, "o1-pro", "openai").unwrap(),
        ("o1-pro".to_string(), Provenance::Cached)
    );
}

#[test]
fn a_nonempty_seed_with_no_match_is_the_seed_verbatim() {
    // The TOTALITY: a present-but-incomplete cache must not veto a model the provider
    // may well accept. A non-empty seed no id contains is passed through unchanged
    // (Verbatim) — a brand-new full id self-heals (tried verbatim → succeeds); a typo
    // is tried verbatim → 404 (the §5.3 caller then runs `bz --list-models`).
    let list = [
        model("claude-opus-4-1", false),
        model("claude-sonnet-4-5", false),
    ];
    assert_eq!(
        select_model(&list, "gpt-5.4", "anthropic").unwrap(),
        ("gpt-5.4".to_string(), Provenance::Verbatim)
    );
}

#[test]
fn a_cold_cache_yields_verbatim_for_any_nonempty_seed() {
    // cache-absent ≡ cache-present-but-empty (§4): an EMPTY list + a non-empty seed is
    // the seed verbatim, never an error — the pre-cache behavior, transparent until
    // `bz --list-models` runs. A full id and a partial both pass through.
    assert_eq!(
        select_model(&[], "gpt-5.4", "openai").unwrap(),
        ("gpt-5.4".to_string(), Provenance::Verbatim)
    );
    assert_eq!(
        select_model(&[], "opus", "anthropic").unwrap(),
        ("opus".to_string(), Provenance::Verbatim)
    );
}

#[test]
fn the_lone_error_is_empty_seed_and_empty_list_config_78() {
    // The ONLY failure (§4): no model given AND no cache to default from → Config (78),
    // the `NoProvider`/`AmbiguousModel` family. A non-empty seed over an empty list is
    // NOT this case (it is Verbatim, above).
    let err = select_model(&[], "", "anthropic").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
    // Pin the EXACT message (spec §4): the verbatim text, NAMING the cold provider so a
    // multi-provider user knows which cache to fill — drift in either direction is caught.
    assert_eq!(
        err.message,
        "no model given and no model cache for anthropic; pass --model or run `bz --list-models`"
    );
}

#[test]
fn provenance_is_copy_eq_debug() {
    // The carried §5.3 fact is a plain Copy enum — round-trips through the serve→404 hint.
    let p = Provenance::Cached;
    assert_eq!(p, p);
    assert_ne!(Provenance::Cached, Provenance::Verbatim);
    assert!(!format!("{p:?}").is_empty());
}
