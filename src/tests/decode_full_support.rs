//! Shared harness for the non-stream `decode_full` golden tests (config §4.2): fold
//! a COMPLETE `stream:false` body, append the one run-owned `End`, and assert the
//! start/stop bracketing invariant. A subdirectory module so cargo does not compile
//! it as its own test binary; `#![allow(dead_code)]` because each split test crate
//! uses only a subset of these helpers.
#![allow(dead_code)]

use crate::{Delta, Event, Protocol};

/// Fold a non-stream body through `decode_full`, append the one run-owned `End`,
/// then assert the start/stop bracketing invariant (the same guard the streaming
/// `golden()` helpers enforce). Returns events + `terminated`.
pub fn full(proto: &dyn Protocol, body: &[u8]) -> (Vec<Event>, bool) {
    let mut state = crate::DecodeState::default();
    let mut events = proto.decode_full(body, &mut state).unwrap();
    events.push(Event::End); // run owns the one terminator; decode_full emits none
    let mut open = std::collections::HashSet::new();
    for e in &events {
        match e {
            Event::ContentStart { index, .. } => assert!(open.insert(*index), "double start"),
            Event::ContentDelta { index, .. } => {
                assert!(open.contains(index), "delta outside block")
            }
            Event::ContentStop { index } => assert!(open.remove(index), "stop without start"),
            _ => {}
        }
    }
    assert!(open.is_empty(), "a content block never closed");
    (events, state.terminated)
}

pub fn jdelta(index: u32, t: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::JsonDelta(t.into()),
    }
}

pub fn tdelta(index: u32, t: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::TextDelta(t.into()),
    }
}
