//! Golden fixture decode for `claude_code` (claude-code spec §8): the REAL captured
//! streams (claude v2.1.217, 2026-07-21, the spec §2 invocation) decode to the
//! canonical grammar, identically under whole-fixture vs one-byte rechunking (arch
//! §9.3). `basic` carries thinking + signature + text + usage + `result`; the
//! logged-out capture is the crisp Auth error. No network, no subprocess.

use crate::protocol::claude_code::ClaudeCode;
use crate::{ContentKind, DecodeState, Delta, ErrorKind, Event, FinishReason, Framing, Protocol};

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/claude_code_basic.ndjson");
const LOGGED_OUT: &[u8] = include_bytes!("../../tests/fixtures/claude_code_error_loggedout.ndjson");

/// Frame the NDJSON bytes (one chunk, or one byte at a time) then decode the whole
/// stream, appending the single run-owned `End`. Returns events + `terminated`.
fn decode_all(bytes: &[u8], one_byte: bool) -> (Vec<Event>, bool) {
    let mut dec = Framing::Ndjson.decoder();
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
        events.extend(ClaudeCode.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator; decode emits none
    (events, state.terminated)
}

/// Decode + assert determinism under one-byte rechunking, then the universal
/// invariants: exactly one End, every `ContentDelta.index` bracketed start..stop.
fn golden(bytes: &[u8]) -> (Vec<Event>, bool) {
    let whole = decode_all(bytes, false);
    assert_eq!(
        decode_all(bytes, true),
        whole,
        "diverged under one-byte rechunk"
    );
    let (events, _) = &whole;
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::End)).count(),
        1,
        "not exactly one End"
    );
    let mut open = std::collections::HashSet::new();
    for e in events {
        match e {
            Event::ContentStart { index, .. } => assert!(open.insert(*index)),
            Event::ContentDelta { index, .. } => {
                assert!(open.contains(index), "delta outside block")
            }
            Event::ContentStop { index } => assert!(open.remove(index)),
            _ => {}
        }
    }
    assert!(open.is_empty(), "a content block never closed");
    whole
}

#[test]
fn the_real_pass_through_stream_decodes_to_the_canonical_grammar() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    // Identity first (spec §5.4): the inner message_start, delegated verbatim.
    let Event::MessageStart { v, id, model, .. } = &ev[0] else {
        panic!("first event must be MessageStart, got {:?}", ev[0])
    };
    assert_eq!(*v, 1);
    assert_eq!(id.as_deref(), Some("msg_011CdGNSjaTsekjiRHfr6Vgr"));
    assert_eq!(model.as_deref(), Some("claude-haiku-4-5-20251001"));
    // Block 0: thinking, streamed as deltas then the opaque signature (bl-61a9).
    assert!(matches!(
        ev.iter().find(|e| matches!(e, Event::ContentStart { .. })),
        Some(Event::ContentStart {
            index: 0,
            kind: ContentKind::Thinking { id: None }
        })
    ));
    let thinking: String = ev
        .iter()
        .filter_map(|e| match e {
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta(t),
            } => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(thinking.contains("respond with \"pong\""));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentDelta { index: 0, delta: Delta::SignatureDelta(s) } if !s.is_empty()
    )));
    // Block 1: the text answer.
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {}
        }
    )));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::ContentDelta { index: 1, delta: Delta::TextDelta(t) } if t == "pong"
    )));
    // Usage is cumulative and Option-shaped — the terminal message_delta's counts.
    let last_usage = ev
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Usage(u) => Some(u.clone()),
            _ => None,
        })
        .expect("a usage event");
    assert_eq!(last_usage.input_tokens, Some(153));
    assert_eq!(last_usage.output_tokens, Some(97));
    // The verdict, then the one End; no Error anywhere on the happy stream.
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::Finish {
            reason: FinishReason::Stop
        }
    )));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error(_))));
    assert_eq!(ev.last(), Some(&Event::End));
}

#[test]
fn the_real_logged_out_stream_is_one_crisp_auth_error() {
    // The CLI never dangles (spec §3.2): logged out is a machine-readable stream —
    // the assistant line's `authentication_failed` tag classifies, the result line
    // folds — one in-band Auth error (77), then End. No MessageStart (error-first).
    let (ev, term) = golden(LOGGED_OUT);
    assert!(term);
    let [Event::Error(e), Event::End] = ev.as_slice() else {
        panic!("expected [Error, End], got {ev:?}")
    };
    assert_eq!(e.kind, ErrorKind::Auth);
    assert_eq!(e.message, "Not logged in · Please run /login");
    let detail = e.provider_detail.as_ref().expect("the result object");
    assert_eq!(detail["type"], "result");
    assert_eq!(detail["is_error"], true);
}

#[test]
fn decode_full_folds_the_same_captures() {
    // The stream:false fold over the SAME real bytes (spec §5.3): same verdicts.
    let mut st = DecodeState::default();
    let ev = ClaudeCode.decode_full(BASIC, &mut st).unwrap();
    assert!(st.terminated);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::Finish {
            reason: FinishReason::Stop
        }
    )));
    let mut st = DecodeState::default();
    let ev = ClaudeCode.decode_full(LOGGED_OUT, &mut st).unwrap();
    assert!(st.terminated);
    assert!(ev
        .iter()
        .any(|e| matches!(e, Event::Error(err) if err.kind == ErrorKind::Auth)));
}

#[test]
fn framing_is_ndjson() {
    assert_eq!(ClaudeCode.framing(), Framing::Ndjson);
}
