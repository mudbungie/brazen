//! The adversarial-rechunking determinism contract (sse §10, arch §9.3) and the
//! universal event invariants (arch §9.2). Every committed fixture is fed through
//! each `Rechunker` strategy and through a synthetic `decode`; the decoded
//! `Vec<Event>` and final `terminated` must be byte-identical across strategies —
//! the decoded stream is a pure function of the input bytes, independent of where
//! the transport cut them. The synthetic decode lives here (concrete protocols are
//! their own tasks); it is enough to exercise the layer's determinism end to end.

use crate::{ContentKind, DecodeState, Delta, Event, Frame, Framing, OpenBlock, Role, Usage};
use serde_json::Value;

// These are SYNTHETIC framer-grammar toys, not real provider wire captures: they
// share one made-up JSON grammar (`usage.input`/`usage.output`, etc.) that the
// `decode_frame` toy below parses, deliberately divergent from any real dialect
// (real Anthropic uses `input_tokens`/`output_tokens`). They exercise only the
// framing/rechunking layer, so they are named for the framing variant each models
// — not a provider. Real fidelity fixtures are the provider-named goldens
// (e.g. `anthropic_messages_basic.sse`), which this test never touches.
const SSE_EVENT: &[u8] = include_bytes!("../../tests/fixtures/synth_frames_sse_event.sse");
const SSE_DONE: &[u8] = include_bytes!("../../tests/fixtures/synth_frames_sse_done.sse");
const NDJSON: &[u8] = include_bytes!("../../tests/fixtures/synth_frames_ndjson.ndjson");
// A PREMATURE SSE stream: message_start + an open content block (start + delta) cut
// BEFORE any content_block_stop / terminal marker — so `terminated` stays false with
// block 0 left open, exercising `run`'s drain-on-error ContentStop synthesis (§5.6).
const SSE_TRUNCATED: &[u8] = include_bytes!("../../tests/fixtures/synth_frames_sse_truncated.sse");

/// Every fixture under test, paired with the framing its `Protocol` declares.
fn fixtures() -> Vec<(&'static str, &'static [u8], Framing)> {
    vec![
        ("sse_event", SSE_EVENT, Framing::Sse),
        ("sse_done", SSE_DONE, Framing::Sse),
        ("ndjson", NDJSON, Framing::Ndjson),
    ]
}

#[derive(Clone, Copy, Debug)]
enum Strategy {
    WholeFixture,
    OneByte,
    MidData,
    MidUtf8,
    MidJsonNumber,
}

const STRATEGIES: [Strategy; 5] = [
    Strategy::WholeFixture,
    Strategy::OneByte,
    Strategy::MidData,
    Strategy::MidUtf8,
    Strategy::MidJsonNumber,
];

/// Cut `bytes` into the chunk sequence a strategy dictates (sse §10).
fn rechunk(bytes: &[u8], strat: Strategy) -> Vec<Vec<u8>> {
    match strat {
        Strategy::WholeFixture => vec![bytes.to_vec()],
        Strategy::OneByte => bytes.iter().map(|b| vec![*b]).collect(),
        Strategy::MidData => split_at(bytes, find(bytes, b"data:").map(|i| i + 6)),
        Strategy::MidUtf8 => split_at(
            bytes,
            bytes.iter().position(|&b| (0x80..=0xBF).contains(&b)),
        ),
        Strategy::MidJsonNumber => split_at(
            bytes,
            bytes
                .windows(2)
                .position(|w| w[0].is_ascii_digit() && w[1].is_ascii_digit())
                .map(|i| i + 1),
        ),
    }
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|w| w == needle)
}

/// Two chunks split at `idx`; a missing/degenerate cut falls back to one chunk so
/// the strategy still runs (the assertion is invariance, not the cut itself).
fn split_at(bytes: &[u8], idx: Option<usize>) -> Vec<Vec<u8>> {
    match idx {
        Some(i) if i > 0 && i < bytes.len() => vec![bytes[..i].to_vec(), bytes[i..].to_vec()],
        _ => vec![bytes.to_vec()],
    }
}

/// Drive `framing`'s framer over the rechunked bytes, then the synthetic decode;
/// return the events (with the single `run`-appended `End`) and final `terminated`.
fn decode_all(bytes: &[u8], framing: Framing, strat: Strategy) -> (Vec<Event>, bool) {
    let mut dec = framing.decoder();
    let mut frames = Vec::new();
    for chunk in rechunk(bytes, strat) {
        frames.extend(dec.push(chunk).unwrap());
    }
    frames.extend(dec.finish().unwrap());

    let mut state = DecodeState::default();
    let mut events = Vec::new();
    for f in &frames {
        events.extend(decode_frame(f, &mut state));
    }
    // `run` owns the terminal injection (arch §4.4, §5.6): on a PREMATURE stream (no
    // decoded terminal marker → `terminated` false) it first CLOSES every still-open
    // block with a synthesized ContentStop — so 'every ContentStart is eventually
    // stopped' holds on FAILURE exactly as on a clean drain — then appends the single
    // End. A clean stream has already drained (open empty), so only End is appended.
    if !state.terminated {
        let mut open: Vec<u32> = state.open.keys().copied().collect();
        open.sort_unstable();
        for index in open {
            events.push(Event::ContentStop { index });
        }
    }
    events.push(Event::End); // `run` owns the single End (arch §4.4); decode never emits it.
    (events, state.terminated)
}

/// A synthetic, vendor-agnostic `decode`: a pure `(Frame, &mut DecodeState)` state
/// machine over the small JSON grammar the fixtures share. It NEVER emits `End` and
/// sets `terminated` only on a terminal marker (`message_stop` / `[DONE]`) — sse §5.
fn decode_frame(frame: &Frame, state: &mut DecodeState) -> Vec<Event> {
    if frame.data == b"[DONE]" {
        state.terminated = true;
        return vec![];
    }
    let v: Value = serde_json::from_slice(&frame.data).unwrap();
    let idx = || v["index"].as_u64().unwrap() as u32;
    match v["type"].as_str().unwrap() {
        "message_start" => vec![Event::message_start(
            v["id"].as_str().map(str::to_owned),
            v["model"].as_str().map(str::to_owned),
            Role::Assistant,
        )],
        "content_block_start" => {
            state.open.insert(
                idx(),
                OpenBlock {
                    kind: ContentKind::Text {},
                },
            );
            vec![Event::ContentStart {
                index: idx(),
                kind: ContentKind::Text {},
            }]
        }
        "content_block_delta" => vec![Event::ContentDelta {
            index: idx(),
            delta: Delta::TextDelta(v["text"].as_str().unwrap().to_owned()),
        }],
        "content_block_stop" => {
            state.open.remove(&idx());
            vec![Event::ContentStop { index: idx() }]
        }
        "message_delta" => {
            let usage = Usage {
                input_tokens: v["usage"]["input"].as_u64().map(|x| x as u32),
                output_tokens: v["usage"]["output"].as_u64().map(|x| x as u32),
                cache_read_tokens: None,
                cache_write_tokens: None,
            };
            vec![Event::Usage(usage)]
        }
        "message_stop" => {
            state.terminated = true;
            vec![]
        }
        _ => vec![],
    }
}

#[test]
fn identical_events_across_every_rechunking() {
    for (name, bytes, framing) in fixtures() {
        let oracle = decode_all(bytes, framing, Strategy::WholeFixture);
        for strat in STRATEGIES {
            let got = decode_all(bytes, framing, strat);
            assert_eq!(got, oracle, "fixture {name} diverged under {strat:?}");
        }
    }
}

#[test]
fn universal_invariants_hold_for_every_fixture() {
    for (name, bytes, framing) in fixtures() {
        let (events, terminated) = decode_all(bytes, framing, Strategy::WholeFixture);

        // The first event of a non-error stream is MessageStart carrying v == 1.
        match &events[0] {
            Event::MessageStart { v, .. } => assert_eq!(*v, 1, "{name}"),
            other => panic!("{name}: first event not MessageStart: {other:?}"),
        }

        // Exactly one End, and it is last; decode itself emitted none.
        assert_eq!(
            events.iter().filter(|e| matches!(e, Event::End)).count(),
            1,
            "{name}: not exactly one End"
        );
        assert!(matches!(events.last(), Some(Event::End)), "{name}");

        // Every ContentDelta.index is bracketed by a ContentStart and a ContentStop.
        let mut open = std::collections::HashSet::new();
        let mut deltas = 0;
        for e in &events {
            match e {
                Event::ContentStart { index, .. } => assert!(open.insert(*index), "{name}"),
                Event::ContentDelta { index, .. } => {
                    assert!(open.contains(index), "{name}: delta outside a block");
                    deltas += 1;
                }
                Event::ContentStop { index } => assert!(open.remove(index), "{name}"),
                _ => {}
            }
        }
        assert!(open.is_empty(), "{name}: a content block never closed");
        assert!(deltas > 0, "{name}: no deltas to bracket");

        // Every clean fixture sets terminated exactly once (on its terminal marker).
        assert!(terminated, "{name}: clean stream left terminated unset");
    }
}

/// The drain-on-error invariant (§5.6): a PREMATURE stream — cut before its terminal
/// marker with a content block still open — STILL brackets every `ContentStart` with a
/// `ContentStop`, because `run` closes the open block before the terminal `End` on
/// failure just as decode does on a clean drain. The error-termination counterpart of
/// `universal_invariants_hold_for_every_fixture` (which pins CLEAN streams only); the
/// synthesized close is a pure function of the bytes — identical across every rechunking.
#[test]
fn open_blocks_close_on_premature_termination() {
    let oracle = decode_all(SSE_TRUNCATED, Framing::Sse, Strategy::WholeFixture);
    let (events, terminated) = &oracle;
    assert!(
        !terminated,
        "a truncated stream must leave terminated unset"
    );

    // Every start is bracketed by a stop even though the wire sent NO content_block_stop
    // and NO terminal marker — the run-owned drain synthesized the missing ContentStop.
    let mut open = std::collections::HashSet::new();
    let mut synthesized_stops = 0;
    for e in events {
        match e {
            Event::ContentStart { index, .. } => assert!(open.insert(*index)),
            Event::ContentStop { index } => {
                assert!(open.remove(index));
                synthesized_stops += 1;
            }
            _ => {}
        }
    }
    assert!(
        open.is_empty(),
        "a content block never closed on premature termination"
    );
    assert!(
        synthesized_stops > 0,
        "the truncated fixture must leave a block for the drain to close"
    );

    // Deterministic across every hostile rechunking, like every other decode.
    for strat in STRATEGIES {
        assert_eq!(
            decode_all(SSE_TRUNCATED, Framing::Sse, strat),
            oracle,
            "premature-termination decode diverged under {strat:?}"
        );
    }
}
