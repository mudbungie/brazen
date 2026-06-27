//! The adversarial-rechunking determinism contract (sse §10, arch §9.3) and the
//! universal event invariants (arch §9.2). Every committed fixture is fed through
//! each `Rechunker` strategy and through a synthetic `decode`; the decoded
//! `Vec<Event>` and final `terminated` must be byte-identical across strategies —
//! the decoded stream is a pure function of the input bytes, independent of where
//! the transport cut them. The synthetic decode lives here (concrete protocols are
//! their own tasks); it is enough to exercise the layer's determinism end to end.

use brazen::{ContentKind, DecodeState, Delta, Event, Frame, Framing, OpenBlock, Role, Usage};
use serde_json::Value;

const ANTHROPIC: &[u8] = include_bytes!("fixtures/sse_anthropic.sse");
const OPENAI: &[u8] = include_bytes!("fixtures/sse_openai.sse");
const OLLAMA: &[u8] = include_bytes!("fixtures/ndjson_ollama.ndjson");

/// Every fixture under test, paired with the framing its `Protocol` declares.
fn fixtures() -> Vec<(&'static str, &'static [u8], Framing)> {
    vec![
        ("anthropic", ANTHROPIC, Framing::Sse),
        ("openai", OPENAI, Framing::Sse),
        ("ollama", OLLAMA, Framing::Ndjson),
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
