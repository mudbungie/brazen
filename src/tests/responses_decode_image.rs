//! Decode coverage for the `openai_responses` `image_generation_call` item (providers
//! §3.4, bl-0987): a model-RETURNED image synthesizes start + one `ImageDelta` + stop
//! from the ONE `output_item.done` frame; the `added` frame and every
//! `response.image_generation_call.*` progress/partial frame open nothing; and the
//! non-stream aggregate folds through the same path. Wire shapes are from the
//! published Responses reference (no live capture — providers §9 CR-Img). No network.

use crate::protocol::openai_responses::OpenAiResponses;
use crate::tests::decode_full_support::{full, tdelta};
use crate::tests::responses_decode_errors_support::{finish_of, run, CREATED};
use crate::{ContentKind, Delta, Event, FinishReason};

const ADDED: &str = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"image_generation_call","id":"ig_1","status":"in_progress","result":null}}"#;

fn image_events(index: u32, media_type: &str, b64: &str) -> Vec<Event> {
    vec![
        Event::ContentStart {
            index,
            kind: ContentKind::Image {
                media_type: media_type.into(),
            },
        },
        Event::ContentDelta {
            index,
            delta: Delta::ImageDelta(b64.into()),
        },
        Event::ContentStop { index },
    ]
}

#[test]
fn the_done_item_synthesizes_start_delta_stop_and_the_added_frame_opens_nothing() {
    let preamble = run(&[CREATED, ADDED]);
    assert_eq!(preamble.len(), 1, "only MessageStart: {preamble:?}");
    let ev = run(&[
        CREATED,
        ADDED,
        r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"image_generation_call","id":"ig_1","status":"completed","output_format":"webp","result":"UklGRg=="}}"#,
    ]);
    assert_eq!(ev[1..], image_events(0, "image/webp", "UklGRg==")[..]);
}

#[test]
fn a_missing_output_format_defaults_to_png() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"image_generation_call","id":"ig_1","status":"completed","result":"iVBORw0KGgo="}}"#,
    ]);
    assert_eq!(ev[1..], image_events(0, "image/png", "iVBORw0KGgo=")[..]);
}

#[test]
fn progress_and_partial_image_frames_are_no_ops() {
    let ev = run(&[
        CREATED,
        ADDED,
        r#"{"type":"response.image_generation_call.in_progress","output_index":0,"item_id":"ig_1"}"#,
        r#"{"type":"response.image_generation_call.generating","output_index":0,"item_id":"ig_1"}"#,
        r#"{"type":"response.image_generation_call.partial_image","output_index":0,"item_id":"ig_1","partial_image_index":0,"partial_image_b64":"PARTIAL=="}"#,
        r#"{"type":"response.image_generation_call.completed","output_index":0,"item_id":"ig_1"}"#,
    ]);
    assert_eq!(ev.len(), 1, "only MessageStart: {ev:?}");
}

#[test]
fn the_image_block_closes_in_place_so_completion_drains_nothing_and_text_follows() {
    let ev = run(&[
        CREATED,
        r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"image_generation_call","id":"ig_1","status":"completed","output_format":"jpeg","result":"/9j/"}}"#,
        r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"message","role":"assistant","content":[]}}"#,
        r#"{"type":"response.content_part.added","output_index":1,"content_index":0,"part":{"type":"output_text","text":""}}"#,
        r#"{"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"Here."}"#,
        r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"message"}}"#,
        r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#,
    ]);
    let mut want = image_events(0, "image/jpeg", "/9j/");
    want.extend([
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {},
        },
        tdelta(1, "Here."),
        Event::ContentStop { index: 1 },
    ]);
    assert_eq!(ev[1..7], want[..]);
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, Event::ContentStop { .. }))
            .count(),
        2
    );
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}

#[test]
fn the_nonstream_aggregate_folds_the_image_item_through_the_same_path() {
    let body = br#"{"id":"resp_1","model":"gpt-5","status":"completed","output":[{"type":"image_generation_call","id":"ig_1","status":"completed","output_format":"png","result":"iVBORw0KGgo="},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}]}"#;
    let (ev, term) = full(&OpenAiResponses, body);
    assert!(term);
    let mut want = image_events(0, "image/png", "iVBORw0KGgo=");
    want.extend([
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {},
        },
        tdelta(1, "Done."),
        Event::ContentStop { index: 1 },
    ]);
    assert_eq!(ev[1..7], want[..]);
    assert_eq!(finish_of(&ev), &FinishReason::Stop);
}
