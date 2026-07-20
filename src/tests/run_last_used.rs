//! The §4 rung-2 ladder, end to end through the real generation path: a provider with
//! no configured default model resolves a BARE `bz` to the model it last used, not to
//! whatever its cached list happens to head with — and an explicit `--model` still wins
//! outright (rung 1 is absolute). Also the write-side steady state: an unchanged cache
//! document is never rewritten. The sibling of `run_learn` (which owns the MEMBERSHIP
//! half — the list append). `MockTransport`/`MemoryModelCache`; zero network.

use std::io::Cursor;

use crate::testing::{MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::Model;

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        default: false,
        ..Default::default()
    }
}

/// A cache primed for `anthropic` as a prior `bz --list-models` would leave it —
/// `claude-opus-…` at the HEAD, so rung 3 would pick it and rung 2 must override.
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
fn re_running_the_same_model_writes_nothing() {
    // The steady state: the pointer already names the model, nothing is learned, so the
    // document would not change — and an unchanged document is not written. No per-request
    // file churn for the overwhelmingly common case.
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = primed().last_used("anthropic", "claude-opus-4-1-20250805");
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
    assert!(
        cache.puts().is_empty(),
        "an unchanged document is never rewritten"
    );
}

#[test]
fn the_pointer_outranks_the_head_on_a_bare_run() {
    // THE MISSING RUNG, end to end: a provider with NO configured default model whose
    // cached list heads with `claude-opus-…`, but whose last successful run used
    // `claude-sonnet-…`. A bare `bz` (empty seed) must now send the SONNET — rung 2
    // (what you used) beating rung 3 (what the provider lists first).
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = primed().last_used("anthropic", "claude-sonnet-4-5-20250929");
    let o = go_cached(
        &["--provider", "anthropic", "--api-key", "sk", "hi"],
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &cache,
    );
    assert_eq!(o.code, 0);
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(
        body.contains("claude-sonnet-4-5-20250929"),
        "the bare run takes the last-used model, not the list head: {body}"
    );
}

#[test]
fn an_explicit_model_still_beats_the_pointer() {
    // Rung 1 is absolute end to end: `--model opus` wins over a `last_used` naming the
    // sonnet. Config encodes INTENT and is never overridden by observation.
    let tx = MockTransport::ok(vec![BASIC]);
    let cache = primed().last_used("anthropic", "claude-sonnet-4-5-20250929");
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
    let body = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(
        body.contains("claude-opus-4-1-20250805"),
        "the explicit seed wins outright: {body}"
    );
}
