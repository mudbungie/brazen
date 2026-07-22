//! Golden fixtures pinning the `v=1` wire serialization across releases (bl-433a).
//!
//! Downstream (lernie) persists brazen's canonical vocabulary VERBATIM: transcripts
//! and `response.json` are JSONL of `v=1` events, and archived workspaces must replay
//! bit-identically years later. So the SERIALIZED shape of every `CanonicalRequest` /
//! `Event` / `CanonicalError` — its tag layout, field names, and optional-field
//! present/absent boundaries — is a frozen contract, not just its typed round-trip.
//!
//! The `golden_v1_*.jsonl` fixtures are checked-in raw wire bytes, one value per line.
//! Each line is deserialized → re-serialized → asserted byte-identical to the checked-in
//! bytes (a serde rename, tag-layout change, or `skip_serializing_if` shift makes the
//! re-serialized bytes drift and fails here), and typed round-tripped. The fixtures are
//! pinned to `EVENT_SCHEMA_VERSION` via the `message_start` `v`, so bumping the constant
//! forces regenerating them. The known-variant wire samples live in `canonical_event`
//! /`canonical_request`; this suite is the cross-release stability net over the whole set.

use crate::{CanonicalError, CanonicalRequest, Event, EVENT_SCHEMA_VERSION};

const EVENTS: &str = include_str!("../../tests/fixtures/golden_v1_events.jsonl");
const ERRORS: &str = include_str!("../../tests/fixtures/golden_v1_errors.jsonl");
const REQUESTS: &str = include_str!("../../tests/fixtures/golden_v1_requests.jsonl");

/// What a future editor sees when a golden stops matching: this is the whole point
/// of the suite — the `v=1` wire shape changed, and downstream transcripts break.
const WIRE_CHANGED: &str = "you changed the v=1 wire serialization of the canonical \
vocabulary — downstream (lernie) persists these bytes and must replay bit-identically. \
If the change is a REMOVAL/RENAME/semantic change: bump EVENT_SCHEMA_VERSION and cut a \
migration note. If it is ADDITIVE (v=1 grows-only): regenerate the golden_v1_*.jsonl \
fixtures to capture the new shape.";

/// Deserialize each golden line into `T`, re-serialize, and assert the bytes are
/// byte-identical to the checked-in line; then assert the typed value round-trips.
fn pin<T>(goldens: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    for (n, line) in goldens.lines().enumerate() {
        let value: T = serde_json::from_str(line).expect(WIRE_CHANGED);
        let reser = serde_json::to_string(&value).expect(WIRE_CHANGED);
        assert_eq!(reser, line, "line {}: {WIRE_CHANGED}", n + 1);
        let back: T = serde_json::from_str(&reser).expect(WIRE_CHANGED);
        assert_eq!(back, value, "line {}: {WIRE_CHANGED}", n + 1);
    }
}

#[test]
fn event_wire_is_pinned() {
    pin::<Event>(EVENTS);
}

#[test]
fn error_wire_is_pinned() {
    pin::<CanonicalError>(ERRORS);
}

#[test]
fn request_wire_is_pinned() {
    pin::<CanonicalRequest>(REQUESTS);
}

#[test]
fn goldens_are_pinned_to_the_current_schema_version() {
    // These goldens were captured against v=1. Bumping the constant is the explicit
    // signal that the vocabulary changed incompatibly; the `message_start` fixtures
    // carry `v == EVENT_SCHEMA_VERSION`, so a bump makes their bytes drift (caught by
    // `event_wire_is_pinned`) unless the fixtures are regenerated for the new version.
    assert_eq!(EVENT_SCHEMA_VERSION, 1, "{WIRE_CHANGED}");
    let mut starts = 0u32;
    for line in EVENTS.lines() {
        if let Event::MessageStart { v, .. } = serde_json::from_str(line).expect(WIRE_CHANGED) {
            starts += 1;
            assert_eq!(v, EVENT_SCHEMA_VERSION, "{WIRE_CHANGED}");
        }
    }
    assert!(starts > 0, "the event goldens must pin v via message_start");
}
