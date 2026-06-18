//! End-to-end `serve` cache lookup (model-discovery §5.2, §5.3, §8): the generation
//! path is a SINGLE send — it reads the per-provider model cache (a local file, no
//! round-trip) and resolves the seed in place, NEVER GETting `/models`. A primed
//! cache expands a partial to its wire id in the encoded body; an empty cache passes a
//! full id through verbatim; `--raw` skips the lookup. A 404 on the generation request
//! is enriched by the carried `Provenance` — a stale-cache hint vs. a not-in-cache hint
//! — both exit 69. `MockTransport`/`MemoryModelCache`; zero network.

mod run_support;

use std::io::Cursor;

use brazen::testing::{Chunk, MemoryModelCache, MockTransport};
use brazen::{Method, Model};
use run_support::*;

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        default: false,
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
fn a_primed_cache_expands_a_partial_in_one_send_no_probe() {
    // `--provider anthropic --model opus` against a PRIMED cache: the partial resolves
    // to the wire id from the cache (a local read), and the chat POST carries it. ONE
    // send — no models GET first (the probe is dissolved).
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_cached(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "opus",
            "--api-key",
            "sk",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &primed(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
    let sent = tx.requests();
    assert_eq!(sent.len(), 1, "one generation send, no probe");
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
    let body = String::from_utf8_lossy(&sent[0].body).into_owned();
    assert!(
        body.contains("claude-opus-4-1-20250805"),
        "the POST carries the expanded wire id: {body}"
    );
    assert!(
        !body.contains("\"opus\""),
        "the seed never reaches the wire"
    );
}

#[test]
fn a_primed_cache_with_an_absent_model_sends_the_default_in_one_send() {
    // No `--model`: the empty seed takes the cache default — `models[0]` (none flags one
    // today, §4) — in a single send.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_cached(
        &["hi", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &primed(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(sent.len(), 1);
    let body = String::from_utf8_lossy(&sent[0].body).into_owned();
    assert!(body.contains("claude-opus-4-1-20250805"));
}

#[test]
fn an_empty_cache_passes_a_full_id_through_verbatim_in_one_send() {
    // An EMPTY (cold) cache + a fully-qualified `--model`: `select_model` is total, so
    // the id passes through verbatim — byte-for-byte the pre-cache behavior, one send.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "claude-x-1",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(
        sent.len(),
        1,
        "no probe — the cold-cache path is one round-trip"
    );
    assert_eq!(sent[0].method, Method::Post);
    let body = String::from_utf8_lossy(&sent[0].body).into_owned();
    assert!(
        body.contains("claude-x-1"),
        "the full id passes through verbatim: {body}"
    );
}

#[test]
fn raw_skips_the_cache_lookup_entirely() {
    // `--raw` bypasses encode and never reads the model, so the lookup is skipped — the
    // user's bytes flow through verbatim. A primed cache would expand `opus`, but raw
    // never consults it; the body is exactly what stdin carried.
    let tx = MockTransport::ok(vec![BASIC]);
    let raw_body = br#"{"model":"opus","messages":[]}"#;
    let o = go_cached(
        &["--raw", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        &mut Cursor::new(raw_body.to_vec()),
        &tx,
        &empty_store(),
        &primed(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].body, raw_body,
        "raw sends the user's bytes verbatim, model unexpanded"
    );
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "opus",
            "--api-key",
            "sk",
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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "typo-model",
            "--api-key",
            "sk",
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

#[test]
fn an_absent_model_against_an_empty_cache_is_config_78() {
    // The lone select_model error (§4): no `--model` AND an empty cache → Config (78),
    // surfaced in-band like any pre-stream error, no send.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go(
        &["hi", "--json", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains(r#""config""#));
    // The message names the cold provider end-to-end (§4): `serve` threads
    // `cfg.provider.name` into `select_model`, so a multi-provider user sees WHICH cache.
    assert!(
        o.stdout
            .contains("no model given and no model cache for anthropic; pass --model"),
        "the in-band message names the cold provider: {}",
        o.stdout
    );
    assert!(
        tx.requests().is_empty(),
        "no generation send on a resolution gap"
    );
}
