//! Shared harness for the `openai_responses` decode-coverage splits (providers
//! §3.4/§3.6/§3.7): the multi-frame `run` folder over one shared `DecodeState`, the
//! `CREATED` preamble, the `finish_of` projector, and the 401 whole-body fixture.
//! A subdirectory module so cargo does not compile it as its own test binary;
//! `#![allow(dead_code)]` because each split test crate uses only a subset.
#![allow(dead_code)]

use crate::protocol::openai_responses::OpenAiResponses;
use crate::{DecodeState, Event, FinishReason, Frame, Protocol};

pub const ERR_401: &[u8] = include_bytes!("../../tests/fixtures/openai_error_401.json");

pub const CREATED: &str =
    r#"{"type":"response.created","response":{"id":"r","model":"m","status":"in_progress"}}"#;

/// Decode a SEQUENCE of frames through ONE shared state (so a block can be opened
/// before a delta routes to it), returning all events concatenated.
pub fn run(frames: &[&str]) -> Vec<Event> {
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

pub fn finish_of(events: &[Event]) -> &FinishReason {
    match events.iter().find(|e| matches!(e, Event::Finish { .. })) {
        Some(Event::Finish { reason }) => reason,
        _ => panic!("no Finish in {events:?}"),
    }
}
