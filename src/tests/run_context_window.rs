//! The context window in-band on `Usage` (model-discovery §5.5): the resolved model
//! row's input-token limit — the DENOMINATOR for the counters — rides every `Usage`
//! event, so a harness that never calls `--list-models` still learns it. Carried off
//! the local cache read `serve` already performs; absent when the row states none, and
//! never fabricated. `MockTransport`/`MemoryModelCache`; zero network.

use std::io::Cursor;

use crate::testing::{MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::Model;

/// A cache row for `anthropic`, optionally stating a window — the only difference
/// between the two arms of the fact (stated / not stated).
fn cache(window: Option<u32>) -> MemoryModelCache {
    MemoryModelCache::with(
        "anthropic",
        vec![Model {
            id: "claude-opus-4-1-20250805".into(),
            default: false,
            context_window: window,
            ..Default::default()
        }],
    )
}

fn go_window(cache: &MemoryModelCache, model: &str, tx: &MockTransport) -> Out {
    go_cached(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            model,
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        &mut Cursor::new(Vec::new()),
        tx,
        &empty_store(),
        cache,
    )
}

#[test]
fn a_stated_window_rides_every_usage_event() {
    // The row states 200000, so BOTH usage events (message_start's input count and
    // message_delta's output count, §3.6) carry the denominator beside the counters —
    // one stamp site, every event, no second call.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(Some(200_000)), "opus", &tx);
    assert_eq!(o.code, 0);
    let usage: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.contains(r#""type":"usage""#))
        .collect();
    assert_eq!(usage.len(), 2, "both usage events: {}", o.stdout);
    for line in &usage {
        assert!(
            line.contains(r#""context_window":200000"#),
            "the window rides the usage event: {line}"
        );
    }
    // Only usage carries it — it is a counter's denominator, not a message fact.
    assert!(
        !o.stdout
            .lines()
            .any(|l| l.contains("context_window") && !l.contains(r#""type":"usage""#)),
        "no other event grows the key: {}",
        o.stdout
    );
}

#[test]
fn a_row_stating_no_window_omits_the_key() {
    // Absent stays absent (the Usage zero-vs-unknown principle): a row whose provider
    // serves no limit leaves the field off the wire entirely — byte-identical to the
    // pre-window event, never a fabricated 0.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(None), "opus", &tx);
    assert_eq!(o.code, 0);
    assert!(
        !o.stdout.contains("context_window"),
        "no key at all when the row states none: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains(r#""type":"usage""#),
        "the usage events are still there: {}",
        o.stdout
    );
}

#[test]
fn a_verbatim_model_the_cache_cannot_place_has_no_window() {
    // The seed resolved `Verbatim` (the cache holds a window, but for a DIFFERENT id):
    // the window belongs to the row, not the provider, so an unplaced id carries none.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(Some(200_000)), "claude-not-in-the-cache", &tx);
    assert_eq!(o.code, 0);
    assert!(
        !o.stdout.contains("context_window"),
        "an unplaced id borrows no other row's window: {}",
        o.stdout
    );
}
