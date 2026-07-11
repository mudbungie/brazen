//! `anthropic_messages` ingress encode, aggregate shape (ingress.md §10, §14): the SAME
//! event fold rendered once at `End` as the non-stream `message` object — the aggregate
//! IS the stream accumulated, no second code path. Byte goldens for the folded body, the
//! §4 `"brazen"` exposure field, refusal `stop_details`, and the egress `decode_full`
//! bridge (the two codecs check each other on the whole-body shape too).

use serde_json::json;

use super::ingress_anthropic_support::{agg_req, encode_all, ms, state};
use super::ingress_encode_support::{json_delta, stop, text_delta, text_start, tool_start};
use crate::protocol::anthropic::AnthropicMessages;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Protocol, Role, Usage};

fn tool_turn() -> Vec<Event> {
    vec![
        ms(),
        text_start(0),
        text_delta(0, "Hello"),
        stop(0),
        tool_start(1, "toolu_a", "get_weather"),
        json_delta(1, "{\"city\":"),
        json_delta(1, "\"SF\"}"),
        stop(1),
        Event::Usage(Usage {
            input_tokens: Some(12),
            output_tokens: Some(8),
            cache_read_tokens: Some(4),
            cache_write_tokens: None,
        }),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        Event::End,
    ]
}

const TOOL_BODY: &str = concat!(
    "{\"content\":[{\"text\":\"Hello\",\"type\":\"text\"},",
    "{\"id\":\"toolu_a\",\"input\":{\"city\":\"SF\"},\"name\":\"get_weather\",\"type\":\"tool_use\"}],",
    "\"id\":\"msg_01XYZ\",\"model\":\"claude-opus-4-8\",\"role\":\"assistant\",",
    "\"stop_reason\":\"tool_use\",\"stop_sequence\":null,\"type\":\"message\",",
    "\"usage\":{\"cache_read_input_tokens\":4,\"input_tokens\":12,\"output_tokens\":8}}",
);

#[test]
fn the_aggregate_is_the_stream_accumulated() {
    let events = tool_turn();
    let mut st = state(agg_req(), &[]);
    // every pre-End event folds silently — zero client bytes until the body
    assert_eq!(encode_all(&events[..events.len() - 1], &mut st), "");
    assert_eq!(encode_all(&events[events.len() - 1..], &mut st), TOOL_BODY);
}

#[test]
fn the_aggregate_survives_the_egress_decode_full() {
    // The whole-body bridge: the egress decoder's explode-and-replay over our folded
    // body reproduces the turn (fragments made whole, the tool input reserialized).
    let mut st = state(agg_req(), &[]);
    let body = encode_all(&tool_turn(), &mut st);
    let mut ds = DecodeState::default();
    let events = AnthropicMessages
        .decode_full(body.as_bytes(), &mut ds)
        .unwrap();
    assert_eq!(
        events,
        vec![
            Event::message_start(
                Some("msg_01XYZ".into()),
                Some("claude-opus-4-8".into()),
                Role::Assistant,
            ),
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(8),
                cache_read_tokens: Some(4),
                cache_write_tokens: None,
            }),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {},
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hello".into()),
            },
            Event::ContentStop { index: 0 },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "toolu_a".into(),
                    name: "get_weather".into(),
                },
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::JsonDelta("{\"city\":\"SF\"}".into()),
            },
            Event::ContentStop { index: 1 },
            Event::Finish {
                reason: FinishReason::ToolUse,
            },
        ]
    );
}

#[test]
fn an_empty_turn_renders_the_null_slots() {
    // Fabricated identity (no MessageStart), empty content, null stop_reason, 0 usage —
    // the wire's own nothing-here spellings.
    let mut st = state(agg_req(), &[]);
    assert_eq!(
        encode_all(&[Event::End], &mut st),
        concat!(
            "{\"content\":[],\"id\":\"msg_brazen-1700000000\",\"model\":\"claude-x\",",
            "\"role\":\"assistant\",\"stop_reason\":null,\"stop_sequence\":null,",
            "\"type\":\"message\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}",
        )
    );
}

#[test]
fn a_refusal_body_carries_stop_details() {
    let mut st = state(agg_req(), &[]);
    let events = [
        ms(),
        Event::Finish {
            reason: FinishReason::Refusal {
                category: "cyber".into(),
                explanation: None,
            },
        },
        Event::End,
    ];
    let v: serde_json::Value = serde_json::from_str(&encode_all(&events, &mut st)).unwrap();
    assert_eq!(v["stop_reason"], json!("refusal"));
    assert_eq!(
        v["stop_details"],
        json!({"category": "cyber", "explanation": null})
    );
    assert_eq!(v["content"], json!([]));
}

#[test]
fn empty_tool_args_and_cache_write_usage() {
    // A tool block with NO input_json_delta → the wire's empty-input `{}`; a usage event
    // carrying a cache-write counter renders `cache_creation_input_tokens`.
    let mut st = state(agg_req(), &[]);
    let events = [
        ms(),
        tool_start(0, "t", "noargs"),
        stop(0),
        Event::Usage(Usage {
            input_tokens: Some(3),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_write_tokens: Some(5),
        }),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        Event::End,
    ];
    let v: serde_json::Value = serde_json::from_str(&encode_all(&events, &mut st)).unwrap();
    assert_eq!(v["content"][0]["input"], json!({})); // empty args → {}
    assert_eq!(
        v["usage"],
        json!({"cache_creation_input_tokens": 5, "input_tokens": 3, "output_tokens": 1})
    );
}

#[test]
fn adaptations_ride_the_top_level_brazen_field() {
    // §4 runtime exposure, aggregate shape: dict-shaped clients see it, typed SDKs drop
    // the unknown field harmlessly; absent when nothing fired.
    let mut st = state(agg_req(), &["thinking_replay"]);
    let body = encode_all(&[ms(), Event::End], &mut st);
    assert!(body.starts_with("{\"brazen\":{\"adaptations\":[\"thinking_replay\"]},"));
    let mut st = state(agg_req(), &[]);
    assert!(!encode_all(&[ms(), Event::End], &mut st).contains("brazen"));
}
