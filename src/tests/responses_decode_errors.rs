//! Decode coverage for `openai_responses` block-routing arms not reached by a full
//! fixture (providers §3.4/§3.6): the `MessageStart` gate, dropped/ignored events,
//! the reasoning-summary + raw + refusal channels, and the content-part routing
//! quirks (late deltas, duplicate opens, untracked closes). The finish-reason,
//! error-envelope, completion-drain, and malformed-frame arms live in the sibling
//! `responses_decode_finish`. No network.

use crate::tests::responses_decode_errors_support::{run, CREATED};

use crate::Event;

#[test]
fn message_start_fires_once_then_in_progress_is_a_noop() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.in_progress","response":{"id":"r","model":"m"}}"#,
    ]);
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, Event::MessageStart { .. }))
            .count(),
        1
    );
}

#[test]
fn a_delta_for_an_unopened_index_is_dropped() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_text.delta","output_index":3,"delta":"x"}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::ContentDelta { .. })));
}

#[test]
fn a_reasoning_item_opens_a_thinking_block_its_summary_delta_routes_in() {
    // The `reasoning` item add opens a Thinking block (identity before content, §3.4),
    // and `reasoning_summary_text.delta` — which carries `summary_index`, no
    // `content_index` → pair (output_index, 0) — routes into it as a ThinkingDelta.
    // Wire shape VERIFIED against OpenAI's published Responses streaming reference
    // (bl-410e / §7 CR-R4): the delta's fields are {item_id, output_index,
    // summary_index, delta, sequence_number} — no content_index. The raw, distinct
    // `response.reasoning_text.delta` channel (content_index-keyed) is handled by its
    // own test below; this one pins the summary channel.
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
        r#"{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"think"}"#,
    ]);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentStart {
            kind: crate::ContentKind::Thinking {},
            ..
        }
    )));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentDelta { delta: crate::Delta::ThinkingDelta(t), .. } if t == "think"
    )));
}

#[test]
fn a_raw_reasoning_text_delta_routes_into_the_thinking_block() {
    // The raw chain-of-thought channel (§3.4, CR-R4): `response.reasoning_text.delta`
    // carries a `content_index` (NOT a `summary_index`). For content_index 0 it routes
    // by pair (output_index, 0) into the Thinking block the `reasoning` item-add opened
    // — no new open logic. Coexistence with the summary channel concatenates into the
    // one block (lossless, redundant); a coexistence rule is deferred to a real capture.
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
        r#"{"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"delta":"raw"}"#,
    ]);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentDelta { delta: crate::Delta::ThinkingDelta(t), .. } if t == "raw"
    )));
}

#[test]
fn inner_reasoning_done_and_part_events_are_no_ops_block_closes_on_item_done() {
    // The inner reasoning `.done`/`.part` family is a DELIBERATE no-op (§3.4, CR-R4),
    // mirroring the content_part.done / output_text.done no-ops: none of
    // reasoning_text.done, reasoning_summary_text.done, reasoning_summary_part.added,
    // or reasoning_summary_part.done opens or closes a block — the Thinking block
    // opened by the `reasoning` item-add is closed exactly once by the outermost
    // `response.output_item.done`. `reasoning_summary_part.added` is the per-part OPEN
    // seam CR-R4 names for a future per-part Thinking block; until a real consumer
    // needs it, the one-block collapse holds and it opens nothing.
    let prefix = &[
        CREATED,
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
        r#"{"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}"#,
        r#"{"type":"response.reasoning_text.done","output_index":0,"content_index":0,"text":"raw"}"#,
        r#"{"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":"sum"}"#,
        r#"{"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"sum"}}"#,
    ][..];

    // Through the inner events: exactly ONE ContentStart (the item-add — part.added
    // opened nothing), and NO ContentStop (no inner `.done` closed the block).
    let inner = run(prefix);
    assert_eq!(
        inner
            .iter()
            .filter(|e| matches!(e, Event::ContentStart { .. }))
            .count(),
        1
    );
    assert!(!inner.iter().any(|e| matches!(e, Event::ContentStop { .. })));

    // The item-level done is what closes it — exactly once, at the canonical index.
    let frames: Vec<&str> = prefix
        .iter()
        .copied()
        .chain(std::iter::once(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning"}}"#,
        ))
        .collect();
    let closed = run(&frames);
    assert_eq!(
        closed
            .iter()
            .filter(|e| matches!(e, Event::ContentStop { index: 0 }))
            .count(),
        1
    );
}

#[test]
fn a_non_output_text_content_part_opens_nothing() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.content_part.added","output_index":0,"part":{"type":"refusal"}}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::ContentStart { .. })));
}

#[test]
fn a_delta_after_its_block_closed_is_dropped() {
    // The (output_index, content_index) pair stays mapped after the item closes
    // (the pair→index map only grows — it is the monotonic index counter), but the
    // block is gone from `open`, so a stray late delta routes nowhere.
    let ev = run(&[
        CREATED,
        r#"{"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text"}}"#,
        r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"message"}}"#,
        r#"{"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"late"}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::ContentDelta { .. })));
}

#[test]
fn a_duplicate_content_part_reuses_its_canonical_index() {
    // A re-`added` pair resolves to the SAME canonical index (robust to a malformed
    // stream that double-opens a part) rather than allocating a second block.
    let ev = run(&[
        CREATED,
        r#"{"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text"}}"#,
        r#"{"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text"}}"#,
    ]);
    let starts: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::ContentStart { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![0, 0]); // both opens land on the one canonical index
}

#[test]
fn output_item_done_for_an_untracked_index_is_ignored() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_item.done","output_index":9,"item":{"type":"message"}}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::ContentStop { .. })));
}
