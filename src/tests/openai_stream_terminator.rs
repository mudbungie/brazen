//! Golden decode for the openai chat STREAM TERMINATOR + in-band error (bl-296d,
//! openai-chat-mapping §3.6/§4.3): a `data: {"error":…}` frame on a 2xx stream is
//! SURFACED (not swallowed), kind-from-body (CR-10); and a non-null `finish_reason`
//! chunk flips `terminated` so a compat server that closes with no `[DONE]` finishes
//! cleanly (no spurious premature-EOF/69). Each fixture decodes identically under
//! whole-fixture vs one-byte rechunking (arch §9.3). The content-shape goldens live in
//! `openai_fixtures`; these pin the terminator seam. No network.

use crate::protocol::openai::OpenAiChat;
use crate::{CanonicalError, ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason};
use crate::{Framing, Protocol, Role};

const MIDSTREAM_ERR_DONE: &[u8] =
    include_bytes!("../../tests/fixtures/openai_chat_midstream_error_done.sse");
const MIDSTREAM_ERR_EOF: &[u8] =
    include_bytes!("../../tests/fixtures/openai_chat_midstream_error_eof.sse");
const FINISH_NO_DONE: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_finish_no_done.sse");

/// Frame the SSE bytes (whole, or one byte at a time) then decode, appending the single
/// run-owned `End`. Returns events + `terminated` (the flag `run` reads to gate the
/// premature-EOF injection, arch §5.6).
fn decode_all(bytes: &[u8], one_byte: bool) -> (Vec<Event>, bool) {
    let mut dec = Framing::Sse.decoder();
    let mut frames = Vec::new();
    if one_byte {
        for b in bytes {
            frames.extend(dec.push(vec![*b]).unwrap());
        }
    } else {
        frames.extend(dec.push(bytes.to_vec()).unwrap());
    }
    frames.extend(dec.finish().unwrap());
    let mut state = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(OpenAiChat.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§3.6); decode emits none
    (events, state.terminated)
}

/// Decode, asserting determinism under adversarial one-byte rechunking (arch §9.3).
fn golden(bytes: &[u8]) -> (Vec<Event>, bool) {
    let whole = decode_all(bytes, false);
    assert_eq!(
        decode_all(bytes, true),
        whole,
        "diverged under one-byte rechunk"
    );
    whole
}

/// The single `Event::Error` in a decoded stream (panics if not exactly one).
fn only_error(ev: &[Event]) -> &CanonicalError {
    let mut it = ev.iter().filter_map(|e| match e {
        Event::Error(e) => Some(e),
        _ => None,
    });
    let e = it.next().expect("expected one Error");
    assert!(it.next().is_none(), "expected exactly one Error");
    e
}

#[test]
fn midstream_error_with_done_surfaces_the_error_and_terminates() {
    // A `data: {"error":…}` frame mid-2xx-stream is SURFACED, not swallowed (the bl-296d
    // bug), kind-from-body (rate_limit_error → Provider{429}, CR-10). The trailing
    // `[DONE]` sets `terminated`, so run appends End with no premature-EOF.
    let (ev, term) = golden(MIDSTREAM_ERR_DONE);
    assert!(term);
    assert!(matches!(
        &ev[..],
        [Event::MessageStart { .. }, Event::Error(_), Event::End]
    ));
    let e = only_error(&ev);
    assert_eq!(e.kind, ErrorKind::Provider { status: 429 });
    assert_eq!(e.exit_code(), 69);
    assert!(e.retryable());
    assert!(e.message.contains("Rate limit"));
    assert!(e.provider_detail.is_some());
    assert!(e.retry_after_seconds.is_none()); // no header on a mid-stream 2xx error
}

#[test]
fn midstream_error_without_done_still_surfaces_leaving_terminated_false() {
    // Same surfacing WITHOUT a trailing `[DONE]`: the error is still emitted (never
    // discarded), kind-from-body (server_error → Provider{500}). `terminated` stays
    // FALSE — an error is not a clean terminal marker (arch §5.6) — so run appends the
    // premature-EOF Transport after it, last-error-wins (§4.3), the honest read.
    let (ev, term) = golden(MIDSTREAM_ERR_EOF);
    assert!(!term);
    assert!(matches!(
        &ev[..],
        [Event::MessageStart { .. }, Event::Error(_), Event::End]
    ));
    let e = only_error(&ev);
    assert_eq!(e.kind, ErrorKind::Provider { status: 500 });
    assert_eq!(e.exit_code(), 70);
    assert!(e.retryable());
    assert!(e.message.contains("server had an error"));
}

#[test]
fn finish_reason_terminates_even_without_done() {
    // The second bl-296d defect: a compat server that closes after the finish chunk with
    // NO `[DONE]`. `finish_reason` now flips `terminated`, so run appends End with no
    // spurious premature-EOF/69 on a clean completion (§3.6). No Error.
    let (ev, term) = golden(FINISH_NO_DONE);
    assert!(term);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error(_))));
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("chatcmpl-f".into()),
                Some("gpt-4o-2024-08-06".into()),
                Role::Assistant,
            ),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {},
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hi".into()),
            },
            Event::ContentStop { index: 0 },
            Event::Finish {
                reason: FinishReason::Stop,
            },
            Event::End,
        ]
    );
}
