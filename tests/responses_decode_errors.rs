//! Decode coverage for `openai_responses` arms not reached by a full fixture
//! (providers §3.4/§3.6/§3.7): the `MessageStart` gate, dropped/ignored events,
//! the reasoning-summary + refusal channels, incomplete/unknown finish reasons, the
//! mid-stream + whole-body error envelopes, and a malformed frame. No network.

use brazen::protocol::openai_responses::OpenAiResponses;
use brazen::{CanonicalError, DecodeState, ErrorKind, Event, FinishReason, Frame, Protocol};

const ERR_401: &[u8] = include_bytes!("fixtures/openai_error_401.json");

/// Decode a SEQUENCE of frames through ONE shared state (so a block can be opened
/// before a delta routes to it), returning all events concatenated.
fn run(frames: &[&str]) -> Vec<Event> {
    let mut state = DecodeState::default();
    let mut out = Vec::new();
    for f in frames {
        let frame = Frame {
            event: None,
            data: f.as_bytes().to_vec(),
            status: None,
        };
        out.extend(OpenAiResponses.decode(frame, &mut state).unwrap());
    }
    out
}

const CREATED: &str =
    r#"{"type":"response.created","response":{"id":"r","model":"m","status":"in_progress"}}"#;

fn finish_of(events: &[Event]) -> &FinishReason {
    match events.iter().find(|e| matches!(e, Event::Finish { .. })) {
        Some(Event::Finish { reason }) => reason,
        _ => panic!("no Finish in {events:?}"),
    }
}

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
            kind: brazen::ContentKind::Thinking {},
            ..
        }
    )));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentDelta { delta: brazen::Delta::ThinkingDelta(t), .. } if t == "think"
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
        Event::ContentDelta { delta: brazen::Delta::ThinkingDelta(t), .. } if t == "raw"
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

#[test]
fn a_streamed_refusal_wins_at_completion() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.refusal.delta","output_index":0,"delta":"I can't help with that."}"#,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert_eq!(
        finish_of(&ev),
        &FinishReason::Refusal {
            category: "refusal".into(),
            explanation: Some("I can't help with that.".into()),
        }
    );
}

#[test]
fn an_unknown_completed_status_is_preserved_verbatim() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.completed","response":{"status":"expired","output":[]}}"#,
    ]);
    assert_eq!(finish_of(&ev), &FinishReason::Other("expired".into()));
}

#[test]
fn completed_without_usage_emits_no_usage_event() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert!(!ev.iter().any(|e| matches!(e, Event::Usage(_))));
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}

#[test]
fn incomplete_maps_length_and_other() {
    let length = run(&[
        CREATED,
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
    ]);
    assert_eq!(finish_of(&length), &FinishReason::Length);
    let other = run(&[
        CREATED,
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}"#,
    ]);
    assert_eq!(
        finish_of(&other),
        &FinishReason::Other("content_filter".into())
    );
}

#[test]
fn mid_stream_errors_decode_kind_from_the_body() {
    // response.failed: a server fault (`code`, nested under response.error) is
    // 5xx-class → Provider{500}/70, NOT a blanket Transport (§3.7, CR-10).
    let failed = run(&[
        CREATED,
        r#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"the model failed"}}}"#,
    ]);
    match failed.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Provider { status: 500 });
            assert_eq!(e.exit_code(), 70);
            assert_eq!(e.message, "the model failed");
            assert!(e.provider_detail.is_some());
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // response.error at top level: a rate limit (`code`) → Provider{429}/69.
    let limited = run(&[
        r#"{"type":"response.error","error":{"code":"rate_limit_exceeded","message":"slow"}}"#,
    ]);
    match limited.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Provider { status: 429 });
            assert_eq!(e.exit_code(), 69);
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // the tag may ride `type` when `code` is absent (the or_else fallback).
    let typed =
        run(&[r#"{"type":"response.error","error":{"type":"rate_limit_error","message":"slow"}}"#]);
    match typed.last() {
        Some(Event::Error(e)) => assert_eq!(e.kind, ErrorKind::Provider { status: 429 }),
        other => panic!("expected Error, got {other:?}"),
    }
    // an unrecognized/absent tag stays retryable Transport (exit 69).
    let untyped = run(&[r#"{"type":"response.error","error":{"message":"boom"}}"#]);
    match untyped.last() {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Transport);
            assert_eq!(e.message, "boom");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn whole_body_error_maps_the_status_family() {
    let frame = Frame {
        event: None,
        data: ERR_401.to_vec(),
        status: Some(401),
    };
    match OpenAiResponses
        .decode(frame, &mut DecodeState::default())
        .unwrap()
        .pop()
    {
        Some(Event::Error(e)) => {
            assert_eq!(e.kind, ErrorKind::Auth);
            assert_eq!(e.exit_code(), 77);
            assert_eq!(e.message, "Incorrect API key provided.");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn an_unknown_event_type_yields_nothing() {
    assert!(run(&[r#"{"type":"response.queued"}"#]).is_empty()); // keep-alive / future type
}

#[test]
fn completion_drains_a_still_open_block() {
    // no output_item.done arrives, so response.completed drains the open text block
    let ev = run(&[
        CREATED,
        r#"{"type":"response.content_part.added","output_index":0,"part":{"type":"output_text"}}"#,
        r#"{"type":"response.output_text.delta","output_index":0,"delta":"hi"}"#,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    assert!(ev
        .iter()
        .any(|e| matches!(e, Event::ContentStop { index: 0 })));
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}

#[test]
fn malformed_frame_surfaces_a_transport_error() {
    let frame = Frame {
        event: None,
        data: b"{not json".to_vec(),
        status: None,
    };
    let err: CanonicalError = OpenAiResponses
        .decode(frame, &mut DecodeState::default())
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}
