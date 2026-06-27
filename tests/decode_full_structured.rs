//! Golden non-stream decode for the EXPLICIT-STRUCTURE dialects (config §4.2):
//! `anthropic` and `openai_responses` carry wire block structure, so `decode_full`
//! explodes each finished `content[]`/`output[]` item into its synthetic
//! start→delta→stop (or added/part/done) frames and replays them through the SAME
//! `decode`-internal handlers — reusing `blocks`/`terminal`/`event` verbatim, no
//! second parser. The structureless dialects live in `decode_full`. No network.

mod decode_full_support;

use brazen::protocol::anthropic::AnthropicMessages;
use brazen::protocol::openai_responses::OpenAiResponses;
use brazen::{ContentKind, Delta, Event, FinishReason, Role, Usage};
use decode_full_support::*;

#[test]
fn anthropic_nonstream_folds_thinking_redacted_text_tool_and_finish() {
    // The whole `message` explodes per `content[i]` block into its
    // start→delta→stop triplet driven through the SAME `blocks` handlers: thinking
    // (signature folds to the buffer, no Delta), redacted_thinking (opens on its
    // `data`, streams NO delta), text, and a tool_use whose whole `input` is one
    // JsonDelta. One Usage carries the full final counts.
    let body = include_bytes!("fixtures/anthropic_messages_nonstream.json");
    let (ev, term) = full(&AnthropicMessages, body);
    assert!(!term); // Messages' terminator is the separate `message_stop`, never in-body
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("msg_01XYZ".into()),
                Some("claude-opus-4-8".into()),
                Role::Assistant,
            ),
            Event::Usage(Usage {
                input_tokens: Some(40),
                output_tokens: Some(20),
                cache_write_tokens: Some(4),
                cache_read_tokens: Some(8),
            }),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking {}
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta("Let me check.".into())
            },
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::RedactedThinking {}
            },
            Event::ContentStop { index: 1 }, // no delta — data folds to the buffer
            Event::ContentStart {
                index: 2,
                kind: ContentKind::Text {}
            },
            tdelta(2, "Hello"),
            Event::ContentStop { index: 2 },
            Event::ContentStart {
                index: 3,
                kind: ContentKind::ToolUse {
                    id: "toolu_01A".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(3, "{\"location\":\"SF\"}"),
            Event::ContentStop { index: 3 },
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}

#[test]
fn anthropic_nonstream_refusal_forwards_stop_details() {
    // A `stop_reason:"refusal"` non-stream body carries its `stop_details` at the
    // top level; the explode→replay reconstructs the terminal `message_delta` with
    // `stop_details` FORWARDED (decode/mod.rs:37), so finish_reason's refusal arm
    // reads category/explanation off it — the non-stream mirror of the STREAMED
    // refusal golden (anthropic_fixtures.rs). An empty `content[]` emits no block
    // events; this asserts the EFFECT of the forwarding line, not just its coverage.
    let body = include_bytes!("fixtures/anthropic_messages_nonstream_refusal.json");
    let (ev, term) = full(&AnthropicMessages, body);
    assert!(!term); // Messages' terminator is the separate `message_stop`, never in-body
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("msg_ref".into()),
                Some("claude-opus-4-8".into()),
                Role::Assistant,
            ),
            Event::Usage(Usage {
                input_tokens: Some(100),
                output_tokens: Some(5),
                cache_write_tokens: None,
                cache_read_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::Refusal {
                    category: "cyber".into(),
                    explanation: Some("Can't help with that.".into()),
                }
            },
            Event::End,
        ]
    );
}

#[test]
fn openai_responses_nonstream_folds_reasoning_multipart_tool_and_finish() {
    // The body IS the `response` object completed wraps. Each `output[oi]` item
    // explodes into its added/part/delta/done frames driven through the SAME `event`
    // dispatcher: a reasoning summary → Thinking, an unknown item → nothing, a
    // two-part message → two Text blocks (distinct (oi,ci)), a function_call → ToolUse
    // with whole arguments. `completed` then drains/usages/finishes verbatim.
    let body = include_bytes!("fixtures/openai_responses_nonstream.json");
    let (ev, term) = full(&OpenAiResponses, body);
    assert!(term); // `response.completed` is the in-body native terminator (§3.4)
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("resp_1".into()),
                Some("gpt-4o-2024-08-06".into()),
                Role::Assistant,
            ),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Thinking {}
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::ThinkingDelta("Thinking.".into())
            },
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::Text {}
            },
            tdelta(1, "Hello"),
            Event::ContentStart {
                index: 2,
                kind: ContentKind::Text {}
            },
            tdelta(2, "Bye"),
            Event::ContentStop { index: 1 },
            Event::ContentStop { index: 2 },
            Event::ContentStart {
                index: 3,
                kind: ContentKind::ToolUse {
                    id: "call_x".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(3, "{\"location\":\"Paris\"}"),
            Event::ContentStop { index: 3 },
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(8),
                cache_read_tokens: Some(0),
                cache_write_tokens: None,
            }),
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}
