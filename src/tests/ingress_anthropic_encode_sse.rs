//! `anthropic_messages` ingress encode, SSE shape (ingress.md §2, §10, §14): canonical
//! event scripts → the anthropic-native SSE stream real SDKs parse — the `event:`/`data:`
//! event framing (`message_start`/`content_block_*`/`message_delta`/`message_stop`),
//! identity fabrication, the `stop_reason` vocabulary, cumulative usage, §4 adaptation
//! comment lines, and the encode→egress-decode round trip. Aggregate shape:
//! `ingress_anthropic_encode_body`; the §9 masquerade: `ingress_anthropic_encode_errors`.

use serde_json::json;

use super::ingress_anthropic_support::{
    egress_decode, encode_all, ms, payloads, state, stream_req,
};
use super::ingress_encode_support::{
    json_delta, sig_delta, stop, text_delta, text_start, think_delta, think_start, tool_start,
};
use crate::{ContentKind, Delta, Event, FinishReason, Role, Usage};

/// Drop the provider-inherent `Usage` events (§5.1 normalize): Anthropic REQUIRES a
/// usage object on `message_start` and `message_delta`, so the encoder fabricates
/// `0`s (the owned masquerade fabrication) that surface as `Usage` on egress decode —
/// dropped wholesale here exactly as the cross-check `normalize` does.
fn no_usage(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .filter(|e| !matches!(e, Event::Usage(_)))
        .collect()
}

fn usage() -> Event {
    Event::Usage(Usage {
        input_tokens: Some(12),
        output_tokens: Some(2),
        cache_read_tokens: None,
        cache_write_tokens: None,
    })
}

#[test]
fn basic_stream_is_the_exact_native_wire() {
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        text_start(0),
        text_delta(0, "Hel"),
        text_delta(0, "lo"),
        stop(0),
        usage(), // usage precedes Finish on the anthropic wire (§3.6, message_delta)
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ];
    // The §3.9 trace, inverted: the anthropic-native `event:`/`data:` framing, the
    // usage-bearing terminal message_delta, and the message_stop terminator.
    let want = concat!(
        "event: message_start\n",
        "data: {\"message\":{\"content\":[],\"id\":\"msg_01XYZ\",\"model\":\"claude-opus-4-8\",\"role\":\"assistant\",\"stop_reason\":null,\"stop_sequence\":null,\"type\":\"message\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}},\"type\":\"message_start\"}\n\n",
        "event: content_block_start\n",
        "data: {\"content_block\":{\"text\":\"\",\"type\":\"text\"},\"index\":0,\"type\":\"content_block_start\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"text\":\"Hel\",\"type\":\"text_delta\"},\"index\":0,\"type\":\"content_block_delta\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"delta\":{\"text\":\"lo\",\"type\":\"text_delta\"},\"index\":0,\"type\":\"content_block_delta\"}\n\n",
        "event: content_block_stop\n",
        "data: {\"index\":0,\"type\":\"content_block_stop\"}\n\n",
        "event: message_delta\n",
        "data: {\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"type\":\"message_delta\",\"usage\":{\"input_tokens\":12,\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    assert_eq!(encode_all(&events, &mut st), want);
    assert_eq!(st.status(), 200); // no Error event → the listener answers 200
}

#[test]
fn encode_then_egress_decode_is_identity() {
    // §14's round-trip property on the response side: the EGRESS decoder run over the
    // ingress-encoded SSE reproduces the canonical script — tool args, thinking with a
    // signature, and a redacted block all carried natively (the stash stays idle).
    let events = vec![
        ms(),
        think_start(0, None),
        think_delta(0, "reason"),
        sig_delta(0, "SIG"),
        stop(0),
        text_start(1),
        text_delta(1, "Answer"),
        stop(1),
        tool_start(2, "toolu_1", "get_weather"),
        json_delta(2, "{\"loc\""),
        json_delta(2, ":\"SF\"}"),
        stop(2),
        Event::ContentStart {
            index: 3,
            kind: ContentKind::RedactedThinking {
                data: "blob".into(),
            },
        },
        stop(3),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        Event::End,
    ];
    let mut st = state(stream_req(), &[]);
    let (got, terminated) = egress_decode(&encode_all(&events, &mut st));
    let mut got = no_usage(got);
    got.push(Event::End); // run appends the one terminator
    assert_eq!(got, events);
    assert!(terminated); // message_stop set terminated: a clean terminal stream
}

#[test]
fn server_tool_blocks_round_trip_natively() {
    // server_tool_use streams like a tool call; the *_tool_result family opens with its
    // full content inline and no delta — both surface through the egress decoder.
    let events = vec![
        ms(),
        Event::ContentStart {
            index: 0,
            kind: ContentKind::ServerToolUse {
                id: "srvtoolu_1".into(),
                name: "web_search".into(),
            },
        },
        json_delta(0, "{\"q\":\"x\"}"),
        stop(0),
        Event::ContentStart {
            index: 1,
            kind: ContentKind::ServerToolResult {
                kind: "web_search_tool_result".into(),
                tool_use_id: "srvtoolu_1".into(),
                content: json!([{"title": "T"}]),
            },
        },
        stop(1),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ];
    let mut st = state(stream_req(), &[]);
    let (got, _) = egress_decode(&encode_all(&events, &mut st));
    let mut got = no_usage(got);
    got.push(Event::End);
    assert_eq!(got, events);
}

#[test]
fn finish_vocabulary_maps_from_canonical_finish() {
    let cases = [
        (FinishReason::Stop, json!("end_turn")),
        (FinishReason::Length, json!("max_tokens")),
        (FinishReason::StopSequence, json!("stop_sequence")),
        (FinishReason::ToolUse, json!("tool_use")),
        (FinishReason::Pause, json!("pause_turn")),
        (FinishReason::Other("bespoke".into()), json!("bespoke")), // verbatim, never panic
    ];
    for (reason, want) in cases {
        let mut st = state(stream_req(), &[]);
        let sse = encode_all(&[Event::Finish { reason }], &mut st);
        assert_eq!(payloads(&sse)[0]["delta"]["stop_reason"], want);
    }
}

#[test]
fn a_streamed_refusal_carries_stop_details() {
    // §3.7 inverted: a refusal is stop_reason:"refusal" + stop_details in the
    // message_delta (NOT an error, NOT a content channel), decoded back to Finish{Refusal}.
    let events = [
        ms(),
        Event::Finish {
            reason: FinishReason::Refusal {
                category: "cyber".into(),
                explanation: Some("cannot help".into()),
            },
        },
        Event::End,
    ];
    let mut st = state(stream_req(), &[]);
    let sse = encode_all(&events, &mut st);
    let md = &payloads(&sse)[1]["delta"];
    assert_eq!(md["stop_reason"], json!("refusal"));
    assert_eq!(
        md["stop_details"],
        json!({"category": "cyber", "explanation": "cannot help"})
    );
    let (events, _) = egress_decode(&sse);
    assert_eq!(
        events.last().unwrap(),
        &Event::Finish {
            reason: FinishReason::Refusal {
                category: "cyber".into(),
                explanation: Some("cannot help".into()),
            },
        }
    );
}

#[test]
fn identity_is_fabricated_when_upstream_names_none() {
    let mut st = state(stream_req(), &[]);
    let events = [Event::message_start(None, None, Role::Assistant)];
    let msg = &payloads(&encode_all(&events, &mut st))[0]["message"];
    assert_eq!(msg["id"], json!("msg_brazen-1700000000")); // Clock-derived, msg_ prefix
    assert_eq!(msg["model"], json!("claude-x")); // the client-requested model
    assert_eq!(msg["usage"], json!({"input_tokens": 0, "output_tokens": 0}));
}

#[test]
fn adaptation_comments_precede_the_first_frame() {
    // §4 runtime exposure, stream shape: SSE comment lines ride just before the first
    // frame — even a chunkless stream (only message_stop) carries them.
    let mut st = state(stream_req(), &["thinking_replay", "document_url_drop"]);
    let sse = encode_all(&[ms()], &mut st);
    assert!(sse.starts_with(
        ": brazen adaptation=thinking_replay\n: brazen adaptation=document_url_drop\nevent: message_start\n"
    ));
    let comments: Vec<&str> = sse.lines().filter(|l| l.starts_with(':')).collect();
    assert_eq!(comments.len(), 2); // exposed once, not re-flushed per frame
    let mut st = state(stream_req(), &["thinking_replay"]);
    assert!(encode_all(&[Event::End], &mut st)
        .starts_with(": brazen adaptation=thinking_replay\nevent: message_stop\n"));
}

#[test]
fn dialectless_events_and_unroutable_deltas_emit_nothing() {
    // A delta whose kind has no anthropic slot (encrypted reasoning, forward-compat),
    // a delta on an unopened index, a stop on an unopened index, an unknown ContentKind,
    // and --raw-only events all fold to zero client bytes.
    let mut st = state(stream_req(), &[]);
    let silent = [
        Event::ContentDelta {
            index: 9,
            delta: Delta::TextDelta("lost".into()),
        },
        stop(7),
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Other(json!({"future": true})),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::EncryptedReasoningDelta("enc".into()),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::Other(json!({"x": 1})),
        },
        stop(0), // a Skip block closes with no wire trace
        Event::Raw(vec![0xFF]),
        Event::Other,
    ];
    for e in &silent {
        assert_eq!(encode_all(std::slice::from_ref(e), &mut st), "", "{e:?}");
    }
}
