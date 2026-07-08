//! End-to-end `serve` cache lookup (model-discovery §5.2, §5.3, §8): the generation
//! path is a SINGLE send — it reads the per-provider model cache (a local file, no
//! round-trip) and resolves the seed in place, NEVER GETting `/models`. A primed
//! cache expands a partial to its wire id in the encoded body; an empty cache passes a
//! full id through verbatim; `--raw` skips the lookup. The 404 `Provenance` hints live
//! in `run_cache_hints`. `MockTransport`/`MemoryModelCache`; zero network.

use std::io::Cursor;

use crate::testing::{MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::{Method, Model};

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
        &["--provider", "anthropic", "--api-key", "sk", "hi"],
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
fn zero_config_routes_to_the_first_provider_and_first_cached_model() {
    // The headline: NO `--provider` and NO `--model` at all (`bz "hi"`). Resolution
    // defaults to the first provider row (anthropic, by name); the empty model seed
    // takes the cache default (`models[0]` = claude-opus-4-1). ONE send, the wire id
    // on the body — the whole zero-config path end to end.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_cached(
        &["--api-key", "sk", "hi"],
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
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
    let body = String::from_utf8_lossy(&sent[0].body).into_owned();
    assert!(
        body.contains("claude-opus-4-1-20250805"),
        "the default provider's first cached model rides the body: {body}"
    );
}

#[test]
fn an_empty_cache_passes_a_full_id_through_verbatim_in_one_send() {
    // An EMPTY (cold) cache + a fully-qualified `--model`: `select_model` is total, so
    // the id passes through verbatim — byte-for-byte the pre-cache behavior, one send.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go(
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
fn an_absent_model_against_an_empty_cache_is_config_78() {
    // The lone select_model error (§4): no `--model` AND an empty cache → Config (78),
    // surfaced in-band like any pre-stream error, no send.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go(
        &["--json", "--provider", "anthropic", "--api-key", "sk", "hi"],
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
