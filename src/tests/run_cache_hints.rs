//! End-to-end `serve` 404 enrichment (model-discovery §5.3): a 404 on the
//! generation request is enriched by the carried `Provenance` — the stale-cache
//! hint on a Cached-resolved model vs. the not-in-cache hint on a Verbatim one —
//! both exit 69, provider body surfaced alongside. The lookup/expansion behaviors
//! live in `run_cache`. Per-file harness copy (`model`/`primed`); zero network.

use std::io::Cursor;

use crate::testing::{Chunk, MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::Model;

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        default: false,
        ..Default::default()
    }
}

/// A cache primed for `anthropic`, newest-first (the provider order), `claude-opus-4-1`
/// first — the seed `opus` expands to it (exact-before-contains, first-in-order, §4).
fn primed() -> MemoryModelCache {
    MemoryModelCache::with(
        "anthropic",
        vec![
            model("claude-opus-4-1-20250805"),
            model("claude-sonnet-4-5-20250929"),
        ],
    )
}

#[test]
fn a_404_on_a_cached_model_is_69_with_the_stale_hint() {
    // A Cached-resolved model (the partial expanded from the primed cache) that 404s →
    // exit 69 + the §5.3 stale-cache hint: we KNOW it was on the list, so re-run
    // list-models. The provider's own diagnostic AND the hint both reach the user.
    let tx = MockTransport::new(
        404,
        vec![Chunk::Data(
            br#"{"error":{"message":"no such model"}}"#.to_vec(),
        )],
    );
    let o = go_cached(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "opus",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &primed(),
    );
    assert_eq!(o.code, 69);
    assert!(
        o.stdout.contains("no such model"),
        "provider body: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("cache may be stale") && o.stdout.contains("list-models"),
        "the stale-cache hint: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("claude-opus-4-1-20250805"),
        "the hint names the resolved wire id: {}",
        o.stdout
    );
}

#[test]
fn a_404_on_a_verbatim_model_is_69_with_the_not_in_cache_hint() {
    // A Verbatim model (an empty cache passes the id through) that 404s → exit 69 + the
    // §5.3 not-in-cache hint: a cold cache or a typo, so run list-models to refresh or
    // enable partial matching.
    let tx = MockTransport::new(
        404,
        vec![Chunk::Data(br#"{"error":{"message":"unknown"}}"#.to_vec())],
    );
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "typo-model",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 69);
    assert!(
        o.stdout.contains("is not in the model cache") && o.stdout.contains("list-models"),
        "the not-in-cache hint: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("typo-model"),
        "the hint names the verbatim seed: {}",
        o.stdout
    );
}
