//! Golden fixture decode for `google_generative_ai` (providers §4.4): each recorded
//! `streamGenerateContent` SSE stream decodes to the exact canonical `Vec<Event>`,
//! identically under whole-fixture vs one-byte rechunking (arch §9.3). The basic
//! fixture's half of the cross-provider cross-check is in `cross_check_basic.rs`.
//! No network.

use crate::protocol::google_genai::GoogleGenAi;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Framing, Protocol, Role, Usage};

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/google_genai_basic.sse");
const TOOLS: &[u8] = include_bytes!("../../tests/fixtures/google_genai_tools.sse");
const THINKING: &[u8] = include_bytes!("../../tests/fixtures/google_genai_thinking.sse");
const PROMPT_BLOCK: &[u8] = include_bytes!("../../tests/fixtures/google_genai_prompt_block.sse");

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
        events.extend(GoogleGenAi.decode(f, &mut state).unwrap());
    }
    events.push(Event::End); // run owns the one terminator (§4.4)
    (events, state.terminated)
}

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

fn start() -> Event {
    Event::message_start(None, Some("gemini-1.5-flash".into()), Role::Assistant)
}

#[test]
fn framing_is_sse() {
    assert_eq!(GoogleGenAi.framing(), Framing::Sse);
}

#[test]
fn basic_text_synthesizes_block_and_finishes_on_the_last_chunk() {
    let (ev, term) = golden(BASIC);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(), // Google streams no id → MessageStart id is None (§4.4)
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hel".into())
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("lo".into())
            },
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input_tokens: Some(5),
                output_tokens: Some(2),
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn whole_function_call_synthesizes_id_and_promotes_to_tool_use() {
    let (ev, term) = golden(TOOLS);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::ToolUse {
                    id: "call_0".into(), // synthesized — Google sends none (§4.5)
                    name: "get_weather".into(),
                },
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::JsonDelta("{\"location\":\"Paris\"}".into()),
            },
            // the functionCall part's thoughtSignature → SignatureDelta on the tool
            // block (bl-61a9); a sink folds it onto Content::ToolUse.signature
            Event::ContentDelta {
                index: 0,
                delta: Delta::SignatureDelta("gSig==".into()),
            },
            Event::ContentStop { index: 0 },
            Event::Usage(Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_read_tokens: Some(3), // cachedContentTokenCount → cache_read (§4.6)
                cache_write_tokens: None,
            }),
            // Google reports STOP even on a tool call; the adapter promotes (§4.7)
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}

#[test]
fn thought_part_routes_to_a_thinking_block_not_the_answer_text() {
    // `parts[].thought == true` (surfaced via `thinkingConfig.includeThoughts`) is
    // private chain-of-thought: a Thinking block + ThinkingDelta, NEVER TextDelta — the
    // plain answer part still opens its own text block alongside (§4.4).
    let (ev, term) = golden(THINKING);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            start(),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking { id: None }
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta("Weighing it".into())
            },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::Text {}
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::TextDelta("Hi".into())
            },
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            Event::Usage(Usage {
                input_tokens: Some(4),
                output_tokens: Some(3),
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::Stop
            },
            Event::End,
        ]
    );
}

#[test]
fn prompt_level_block_finishes_as_refusal_not_premature_eof() {
    // A candidate-less chunk carrying `promptFeedback.blockReason` is a deterministic
    // refusal of the PROMPT (HTTP 200, exit 0) — it must `Finish{Refusal}` and set
    // `terminated`, NOT fall through to a premature-EOF Transport/69 (§4.4).
    let (ev, term) = golden(PROMPT_BLOCK);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            // no `modelVersion` on the block chunk → MessageStart model is None too
            Event::message_start(None, None, Role::Assistant),
            Event::Finish {
                reason: FinishReason::Refusal {
                    category: "safety".into(),
                    explanation: None,
                }
            },
            Event::End,
        ]
    );
}
