//! The output-cap / thinking-budget coupling (providers.md §6, config §4.1.2): both
//! budget wires carve thinking OUT of the cap, so `couple_budget` raises a STATED cap
//! to `budget + headroom` and fills an ABSENT one only from the model's SERVED
//! `max_output_tokens` — never invented. Unit arms on the pure fn, then the funnel
//! end to end (`MockTransport`/`MemoryModelCache`; zero network).

use std::io::Cursor;

use serde_json::{json, Value};

use crate::testing::{MemoryModelCache, MockTransport};
use crate::tests::run_support::*;
use crate::{couple_budget, CanonicalRequest, Model, ReasoningEffort};

fn req(max_tokens: Option<u32>, reasoning: Option<ReasoningEffort>) -> CanonicalRequest {
    let mut r: CanonicalRequest = serde_json::from_value(json!({"model": "m"})).unwrap();
    r.max_tokens = max_tokens;
    r.reasoning = reasoning;
    r
}

fn coupled(
    max: Option<u32>,
    effort: Option<ReasoningEffort>,
    budget: bool,
    served: Option<u32>,
) -> Option<u32> {
    let mut r = req(max, effort);
    couple_budget(&mut r, budget, served);
    r.max_tokens
}

#[test]
fn a_stated_cap_is_raised_to_the_floor_never_lowered() {
    use ReasoningEffort::*;
    // The anthropic row default 4096 under each rung → budget + 4096.
    assert_eq!(coupled(Some(4096), Some(Low), true, None), Some(5120));
    assert_eq!(coupled(Some(4096), Some(Medium), true, None), Some(12288));
    assert_eq!(coupled(Some(4096), Some(High), true, None), Some(28672));
    // A generous stated cap is kept, and the served max never overrides a stated one.
    assert_eq!(
        coupled(Some(100_000), Some(High), true, Some(64_000)),
        Some(100_000)
    );
    assert_eq!(
        coupled(Some(1100), Some(Medium), true, Some(64_000)),
        Some(12288)
    );
}

#[test]
fn an_absent_cap_fills_from_the_served_max_else_stays_absent() {
    use ReasoningEffort::*;
    assert_eq!(
        coupled(None, Some(Medium), true, Some(64_000)),
        Some(64_000)
    );
    // A served max under the floor is still floored (the budget wire needs room).
    assert_eq!(coupled(None, Some(High), true, Some(8192)), Some(28672));
    // Nothing served: brazen invents nothing.
    assert_eq!(coupled(None, Some(High), true, None), None);
}

#[test]
fn no_budget_dialect_or_no_reasoning_leaves_the_cap_alone() {
    use ReasoningEffort::*;
    assert_eq!(
        coupled(Some(100), Some(High), false, Some(64_000)),
        Some(100)
    );
    assert_eq!(coupled(None, Some(High), false, Some(64_000)), None);
    assert_eq!(coupled(Some(100), None, true, Some(64_000)), Some(100));
    assert_eq!(coupled(None, None, true, Some(64_000)), None);
}

/// The one body the mock saw.
fn sent(tx: &MockTransport) -> Value {
    let reqs = tx.requests();
    assert_eq!(reqs.len(), 1);
    serde_json::from_slice(&reqs[0].body).unwrap()
}

/// A google row turn with `extra` flags, the cache holding `gemini-x` (served max as given).
fn google(served: Option<u32>, flags: &[&str]) -> Value {
    let cfg = temp(GOOGLE_ROW);
    let cache = MemoryModelCache::with(
        "google",
        vec![Model {
            id: "gemini-x".into(),
            max_output_tokens: served,
            ..Default::default()
        }],
    );
    let tx = MockTransport::ok(vec![b"data: {}\n\n"]);
    let path = cfg.0.to_str().unwrap();
    let mut argv = vec!["--config", path, "--provider", "google", "-m", "gemini-x"];
    argv.extend_from_slice(flags);
    argv.extend_from_slice(&["--api-key", "k", "--json", "hi"]);
    go_cached(
        &argv,
        &[],
        &mut Cursor::new(Vec::new()),
        &tx,
        &empty_store(),
        &cache,
    );
    sent(&tx)["generationConfig"].clone()
}

#[test]
fn the_google_wire_fills_from_the_served_max_and_floors_a_stated_cap() {
    // Absent cap + reasoning: the served max becomes the explicit cap (Claude behind
    // Cloud Code 400s on the backend's small default otherwise, bl-0bc9).
    let g = google(Some(64_000), &["--reasoning", "medium"]);
    assert_eq!(g["maxOutputTokens"], json!(64_000));
    assert_eq!(g["thinkingConfig"]["thinkingBudget"], json!(8192));
    // Nothing served → nothing invented.
    assert!(google(None, &["--reasoning", "medium"])
        .get("maxOutputTokens")
        .is_none());
    // No reasoning → the served max is not a fill (the coupling alone reads it).
    assert!(google(Some(64_000), &[]).is_null());
    // A stated cap under the floor is raised (Gemini would spend it on thoughts).
    let g = google(None, &["--reasoning", "medium", "--max-tokens", "1100"]);
    assert_eq!(g["maxOutputTokens"], json!(12288));
}

#[test]
fn the_anthropic_row_default_is_floored_in_the_funnel() {
    let tx = ok_basic();
    let argv = [
        "--provider",
        "anthropic",
        "-m",
        "claude-x",
        "--reasoning",
        "high",
        "--api-key",
        "sk",
        "hi",
    ];
    go(&argv, &[], b"", &tx, &empty_store());
    let b = sent(&tx);
    assert_eq!(b["max_tokens"], json!(28672)); // row default 4096 → 24576 + 4096
    assert_eq!(b["thinking"]["budget_tokens"], json!(24576));
}
