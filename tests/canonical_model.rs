//! Canonical model tests (§3, §4): the `Model` serde projection and the pure
//! `select_model` resolver — empty-seed default, exact-before-contains partial
//! matching, list order as the authoritative tiebreak, and the `Config` (78)
//! failures. Pure, from literal inputs — no network.

use brazen::{select_model, ErrorKind, Model};
use serde_json::json;

fn model(id: &str, default: bool) -> Model {
    Model {
        id: id.into(),
        default,
    }
}

#[test]
fn model_serializes_serde_direct_and_roundtrips() {
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
fn empty_seed_picks_the_flagged_default_else_first_in_list() {
    // No flag: the FIRST in list order is the default (§4 first-in-list rule).
    let list = [model("a", false), model("b", false)];
    assert_eq!(select_model(&list, "").unwrap(), "a");

    // A later model flagged `default` wins over list order (carried, not invented).
    let flagged = [model("a", false), model("b", true), model("c", false)];
    assert_eq!(select_model(&flagged, "").unwrap(), "b");
}

#[test]
fn nonempty_seed_is_exact_before_contains_first_in_order() {
    let list = [
        model("claude-opus-4-1", false),
        model("claude-opus-4-0", false),
        model("claude-sonnet-4-5", false),
    ];
    // Exact id wins even though it also "contains" itself.
    assert_eq!(
        select_model(&list, "claude-opus-4-0").unwrap(),
        "claude-opus-4-0"
    );
    // Partial: the FIRST (list order) whose id contains the seed — "the suggested
    // version" — even though two ids contain "opus".
    assert_eq!(select_model(&list, "opus").unwrap(), "claude-opus-4-1");
    // Case-insensitive contains.
    assert_eq!(select_model(&list, "OPUS").unwrap(), "claude-opus-4-1");
    assert_eq!(select_model(&list, "SONNET").unwrap(), "claude-sonnet-4-5");
}

#[test]
fn exact_match_resolves_an_unowned_full_id_to_itself() {
    // An id outside the prefix family resolves to itself when the probe confirms it
    // exists, rather than to a longer id that merely contains it (exact-before-contains).
    let list = [model("o1-pro", false), model("o1-pro-2024", false)];
    assert_eq!(select_model(&list, "o1-pro").unwrap(), "o1-pro");
}

#[test]
fn empty_list_is_a_config_error_for_either_seed() {
    for seed in ["", "opus"] {
        let err = select_model(&[], seed).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Config);
        assert_eq!(err.exit_code(), 78);
    }
}

#[test]
fn unmatched_seed_is_config_naming_the_seed_and_a_few_ids() {
    let list = [
        model("claude-opus-4-1", false),
        model("claude-sonnet-4-5", false),
        model("claude-haiku-4-5", false),
        model("claude-3-5-sonnet", false),
    ];
    let err = select_model(&list, "gpt-5").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
    // The message names the unmatched seed and the first three ids, then `…`.
    assert!(
        err.message.contains("gpt-5"),
        "names the seed: {}",
        err.message
    );
    assert!(err.message.contains("claude-opus-4-1"));
    assert!(
        err.message.contains('…'),
        "elides the tail: {}",
        err.message
    );
    assert!(
        !err.message.contains("claude-3-5-sonnet"),
        "bounded to three: {}",
        err.message
    );
}

#[test]
fn unmatched_seed_message_shows_all_ids_when_three_or_fewer() {
    let list = [model("a-model", false), model("b-model", false)];
    let err = select_model(&list, "zzz").unwrap_err();
    assert!(err.message.contains("a-model") && err.message.contains("b-model"));
    assert!(
        !err.message.contains('…'),
        "no elision under four: {}",
        err.message
    );
}
