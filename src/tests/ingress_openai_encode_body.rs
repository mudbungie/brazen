//! `openai_chat` ingress encode, aggregate shape (ingress.md §10, §14): the
//! SAME event fold rendered once at `End` — the aggregate IS the stream
//! accumulated, no second code path. Byte goldens for the folded
//! `chat.completion` body, the §4 `"brazen"` exposure field, the null-slot
//! spellings, and the egress `decode_full` bridge (the two codecs check each
//! other on the whole-body shape too).

use serde_json::json;

use super::ingress_encode_support::{
    agg_req, encode_all, json_delta, ms, state, stop, text_delta, text_start, tool_start,
};
use crate::protocol::openai::OpenAiChat;
use crate::{ContentKind, DecodeState, Delta, Event, FinishReason, Protocol, Role, Usage};

fn tool_turn() -> Vec<Event> {
    vec![
        ms(),
        text_start(0),
        text_delta(0, "Hello"),
        tool_start(1, "call_a", "get_weather"),
        json_delta(1, "{\"city\":"),
        json_delta(1, "\"SF\"}"),
        stop(0),
        stop(1),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        Event::Usage(Usage {
            input_tokens: Some(12),
            output_tokens: Some(8),
            cache_read_tokens: Some(4),
            cache_write_tokens: None,
        }),
        Event::End,
    ]
}

const TOOL_BODY: &str = concat!(
    "{\"choices\":[{\"finish_reason\":\"tool_calls\",\"index\":0,\"message\":",
    "{\"content\":\"Hello\",\"refusal\":null,\"role\":\"assistant\",\"tool_calls\":",
    "[{\"function\":{\"arguments\":\"{\\\"city\\\":\\\"SF\\\"}\",\"name\":\"get_weather\"},",
    "\"id\":\"call_a\",\"type\":\"function\"}]}}],\"created\":1700000000,",
    "\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion\",",
    "\"usage\":{\"completion_tokens\":8,\"prompt_tokens\":12,",
    "\"prompt_tokens_details\":{\"cached_tokens\":4},\"total_tokens\":20}}",
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
    // The whole-body bridge: the egress decoder's explode-and-replay over our
    // folded body reproduces the turn (fragments made whole, indexes intact).
    let mut st = state(agg_req(), &[]);
    let body = encode_all(&tool_turn(), &mut st);
    let mut ds = DecodeState::default();
    let events = OpenAiChat.decode_full(body.as_bytes(), &mut ds).unwrap();
    assert_eq!(
        events,
        vec![
            Event::message_start(
                Some("chatcmpl-9".into()),
                Some("gpt-4o-2024-08-06".into()),
                Role::Assistant,
            ),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {},
            },
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hello".into()),
            },
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "call_a".into(),
                    name: "get_weather".into(),
                },
            },
            Event::ContentDelta {
                index: 1,
                delta: Delta::JsonDelta("{\"city\":\"SF\"}".into()),
            },
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            Event::Finish {
                reason: FinishReason::ToolUse,
            },
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(8),
                cache_read_tokens: Some(4),
                cache_write_tokens: None,
            }),
        ]
    );
}

#[test]
fn an_empty_turn_renders_the_null_slots() {
    // Fabricated identity (no MessageStart), null content/refusal/finish/usage —
    // the wire's own nothing-here spellings, never "" or 0.
    let mut st = state(agg_req(), &[]);
    assert_eq!(
        encode_all(&[Event::End], &mut st),
        concat!(
            "{\"choices\":[{\"finish_reason\":null,\"index\":0,\"message\":",
            "{\"content\":null,\"refusal\":null,\"role\":\"assistant\"}}],",
            "\"created\":1700000000,\"id\":\"chatcmpl-brazen-1700000000\",",
            "\"model\":\"gpt-4o\",\"object\":\"chat.completion\",\"usage\":null}",
        )
    );
}

#[test]
fn a_refusal_lands_on_the_message_refusal_field() {
    let mut st = state(agg_req(), &[]);
    let events = [
        ms(),
        Event::Finish {
            reason: FinishReason::Refusal {
                category: "refusal".into(),
                explanation: Some("cannot help".into()),
            },
        },
        // an all-None usage renders the wire's required-integer slots as 0 —
        // an owned masquerade fabrication (the canonical facts stay None)
        Event::Usage(Usage::default()),
        Event::End,
    ];
    let v: serde_json::Value = serde_json::from_str(&encode_all(&events, &mut st)).unwrap();
    let msg = &v["choices"][0]["message"];
    assert_eq!(msg["refusal"], json!("cannot help"));
    assert_eq!(msg["content"], json!(null));
    assert_eq!(v["choices"][0]["finish_reason"], json!("stop"));
    assert_eq!(
        v["usage"],
        json!({"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0})
    );
}

#[test]
fn adaptations_ride_the_top_level_brazen_field() {
    // §4 runtime exposure, aggregate shape: dict-shaped clients see it,
    // strictly-typed SDKs drop the unknown field harmlessly.
    let mut st = state(agg_req(), &["thinking_replay"]);
    let body = encode_all(&[ms(), Event::End], &mut st);
    assert!(body.starts_with("{\"brazen\":{\"adaptations\":[\"thinking_replay\"]},"));
    // and absent when nothing fired (never an empty stub)
    let mut st = state(agg_req(), &[]);
    let body = encode_all(&[ms(), Event::End], &mut st);
    assert!(!body.contains("brazen"));
}

#[test]
fn include_usage_is_a_stream_knob_only() {
    // stream:false + stream_options.include_usage: the Usage event emits no
    // frame (there is no stream), but the fold still carries it into the body.
    let mut st = state(
        json!({"model": "gpt-4o", "messages": [], "stream": false,
            "stream_options": {"include_usage": true}}),
        &[],
    );
    let events = [
        ms(),
        Event::Usage(Usage {
            input_tokens: Some(3),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::End,
    ];
    let pre = encode_all(&events[..2], &mut st);
    assert_eq!(pre, "");
    let v: serde_json::Value = serde_json::from_str(&encode_all(&events[2..], &mut st)).unwrap();
    // no cache fact → no prompt_tokens_details (absent, never a fabricated 0)
    assert_eq!(
        v["usage"],
        json!({"completion_tokens": 1, "prompt_tokens": 3, "total_tokens": 4})
    );
}
