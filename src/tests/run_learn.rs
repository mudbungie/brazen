//! Learn-the-model-on-success (model-discovery §5.4): the generation data plane is a
//! SECOND writer of the per-provider model cache, the sibling of OAuth refresh's cred
//! write. ONE write site keeps TWO facts in step: MEMBERSHIP — a 2xx on a VERBATIM model
//! (one the cache could not place, yet the provider accepted) appends it to the list
//! tail — and RECENCY — every 2xx points `last_used` at the model it used, which is what
//! a later bare `bz` (empty seed) resolves to (§4 rung 2). The write is guarded on an
//! ACTUAL CHANGE, so re-running the same model writes nothing; a non-2xx never writes.
//! `list-models` stays the authoritative WHOLESALE writer of the list; this only fills a
//! gap discovery left or could not run (e.g. a provider whose `/models` is broken).
//! `MockTransport`/`MemoryModelCache`; zero network.

use std::io::Cursor;

use crate::testing::{Chunk, MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::{CachedModels, Model};

/// The document the write site produces: a list plus the pointer at `last`.
fn doc(models: Vec<Model>, last: &str) -> CachedModels {
    CachedModels {
        models,
        last_used: Some(last.into()),
    }
}

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        default: false,
        ..Default::default()
    }
}

/// A cache primed for `anthropic` (as a prior `bz --list-models` would leave it),
/// `claude-opus-4-1` first — the empty-seed default and the partial-match corpus (§4).
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
fn a_verbatim_success_seeds_a_cold_cache() {
    // The codex-shaped case: a COLD cache (no `--list-models` ever ran / it is broken)
    // and a full `--model` the cache cannot place → sent Verbatim. A 2xx writes it, so
    // the cold cache is now seeded with exactly the one model that worked.
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = MemoryModelCache::new();
    let o = go_cached(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x-1",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &cache,
    );
    assert_eq!(o.code, 0);
    let puts = cache.puts();
    assert_eq!(puts.len(), 1, "the 2xx learned exactly one model");
    assert_eq!(puts[0].0, "anthropic", "into the routed provider's cache");
    assert_eq!(
        puts[0].1,
        doc(vec![model("claude-x-1")], "claude-x-1"),
        "the seeded list is the model that worked, and the pointer names it"
    );
}

#[test]
fn a_learned_model_becomes_the_empty_seed_default_next_run() {
    // The headline — the whole `bz yo` "just works" fix, end to end. Run 1 is an
    // explicit `--model` against a cold cache that 2xx's (the user's working
    // `--provider … --model … yo`). Run 2 is the BARE invocation (no `--model`) sharing
    // the same cache: the empty seed takes the learned model as the default and rides the
    // body — no "no model cache" error, no flags.
    let cache = MemoryModelCache::new();

    let tx1 = MockTransport::ok(vec![BASIC]);
    let first = go_cached(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x-1",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx1,
        &empty_store(),
        &cache,
    );
    assert_eq!(first.code, 0);

    let tx2 = MockTransport::ok(vec![BASIC]);
    let bare = go_cached(
        &["--provider", "anthropic", "--api-key", "sk", "hi again"],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx2,
        &empty_store(),
        &cache,
    );
    assert_eq!(
        bare.code, 0,
        "the bare run no longer hits the cold-cache error"
    );
    let body = String::from_utf8_lossy(&tx2.requests()[0].body).into_owned();
    assert!(
        body.contains("claude-x-1"),
        "the learned model is the empty-seed default on the next run: {body}"
    );
}

#[test]
fn a_cached_model_success_moves_the_pointer_without_touching_the_list() {
    // A partial (`opus`) the PRIMED cache expands to a wire id is `Cached` — already in
    // the list, so nothing is APPENDED — but it is still a model you just used, so the
    // pointer moves to it (§4 rung 2). The list is byte-identical: append-never-reorder
    // holds because the pointer is beside the list, not a permutation of it.
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = primed();
    let o = go_cached(
        &[
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
        &cache,
    );
    assert_eq!(o.code, 0);
    let puts = cache.puts();
    assert_eq!(puts.len(), 1, "the pointer move is the only write");
    assert_eq!(
        puts[0].1,
        doc(
            vec![
                model("claude-opus-4-1-20250805"),
                model("claude-sonnet-4-5-20250929"),
            ],
            "claude-opus-4-1-20250805",
        ),
        "the list is untouched; only `last_used` changed"
    );
}

#[test]
fn a_verbatim_success_appends_preserving_the_existing_list() {
    // A NEW model the primed list cannot place, sent Verbatim, that 2xx's is APPENDED —
    // the discovered order (and its first-in-list default) is preserved, the new id is
    // added to the tail so it is partial-matchable next time. `list-models` order stays
    // authoritative; learning only fills the gap.
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = primed();
    let o = go_cached(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-haiku-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &cache,
    );
    assert_eq!(o.code, 0);
    let puts = cache.puts();
    assert_eq!(puts.len(), 1);
    assert_eq!(
        puts[0].1,
        doc(
            vec![
                model("claude-opus-4-1-20250805"),
                model("claude-sonnet-4-5-20250929"),
                model("claude-haiku-x"),
            ],
            "claude-haiku-x",
        ),
        "the new model is appended and pointed at; the existing order survives"
    );
}

#[test]
fn a_non_2xx_never_learns_the_model() {
    // A Verbatim model that 404s is NOT learned: only a 2xx is evidence the model works.
    // (The 404 still surfaces the §5.3 not-in-cache hint — asserted in run_cache.)
    let tx = MockTransport::new(
        404,
        vec![Chunk::Data(br#"{"error":{"message":"unknown"}}"#.to_vec())],
    );
    let cache = MemoryModelCache::new();
    let o = go_cached(
        &[
            "--provider",
            "anthropic",
            "--model",
            "typo-model",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &cache,
    );
    assert_eq!(o.code, 69);
    assert!(
        cache.puts().is_empty(),
        "a non-2xx is no evidence — nothing is learned"
    );
}
