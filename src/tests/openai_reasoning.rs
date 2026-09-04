//! Golden decode for the openai chat REASONING channel (openai-chat-mapping §3.3a,
//! bl-b68b): `delta.reasoning_content` — the de-facto convention of the
//! openai-compatible class (DeepSeek et al.) — opens a lazy `Thinking` block AHEAD of
//! the answer's text block and streams `ThinkingDelta`, and the non-stream fold
//! projects `message.reasoning_content` onto the same synthetic delta so `stream:false`
//! yields the same canonical shape. Stock OpenAI never sends the field: absent, this
//! decodes nothing (the empty-set path) — pinned by the §3.3a-silent goldens in
//! `openai_fixtures`. Both fixtures decode identically under whole-fixture vs one-byte
//! rechunking (arch §9.3). Synthetic (no DeepSeek key on the test box), authored from
//! the published wire reference. No network.

use crate::protocol::openai::OpenAiChat;
use crate::tests::decode_full_support::full;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};

const REASONING: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_reasoning.sse");
const NONSTREAM: &[u8] =
    include_bytes!("../../tests/fixtures/openai_chat_nonstream_reasoning.json");

/// Frame the SSE bytes (whole, or one byte at a time) then decode, appending the single
/// run-owned `End`. Returns events + `terminated`.
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

fn start() -> Event {
    Event::message_start(
        Some("chatcmpl-dr".into()),
        Some("deepseek-reasoner".into()),
        Role::Assistant,
    )
}
fn think(i: u32, t: &str) -> Event {
    Event::ContentDelta {
        index: i,
        delta: Delta::ThinkingDelta(t.into()),
    }
}

/// The canonical block shape both fixtures share: thinking at index 0 (opened first —
/// reasoning precedes the answer), text at index 1, both drained ascending at finish.
fn blocks(thinking: Vec<Event>) -> Vec<Event> {
    let mut ev = vec![
        start(),
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Thinking { id: None }, // Chat Completions has no reasoning-item id
        },
    ];
    ev.extend(thinking);
    ev.extend([
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {},
        },
        Event::ContentDelta {
            index: 1,
            delta: Delta::TextDelta("Four.".into()),
        },
        Event::ContentStop { index: 0 }, // the drain is ascending (§3.3)
        Event::ContentStop { index: 1 },
    ]);
    ev
}

#[test]
fn reasoning_content_opens_a_thinking_block_before_the_text_block() {
    let whole = decode_all(REASONING, false);
    assert_eq!(
        decode_all(REASONING, true),
        whole,
        "diverged under one-byte rechunk"
    );
    let (ev, term) = whole;
    assert!(term);
    let mut want = blocks(vec![think(0, "Two plus "), think(0, "two is four.")]);
    want.extend([
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ]);
    // the role-only chunk's empty `reasoning_content:""` opens NO block (determinism,
    // §3.3a), and the `reasoning_content:null` on the answer chunk decodes nothing
    assert_eq!(ev, want);
}

#[test]
fn nonstream_reasoning_content_folds_to_the_same_thinking_block() {
    let (ev, term) = full(&OpenAiChat, NONSTREAM);
    assert!(term); // the folded body's non-null finish_reason is a terminal marker (§3.6)
    let mut want = blocks(vec![think(0, "Two plus two is four.")]);
    want.extend([
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::Usage(Usage {
            input_tokens: Some(9),
            output_tokens: Some(7),
            cache_read_tokens: None,
            cache_write_tokens: None,
            ..Default::default()
        }),
        Event::End,
    ]);
    assert_eq!(ev, want);
}
