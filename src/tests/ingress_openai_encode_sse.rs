//! `openai_chat` ingress encode, SSE shape (ingress.md §2, §10, §14): canonical
//! event scripts → byte goldens real SDKs parse — fabricated-but-well-formed
//! identity on every chunk, role on the first delta, index-carrying tool-call
//! deltas pinned against the captured OpenAI transcript, the `finish_reason`
//! vocabulary, usage iff the client's `stream_options.include_usage` asked, the
//! `[DONE]` sentinel, §4 adaptation comment lines, and the encode→egress-decode
//! round trip. Aggregate shape: `ingress_openai_encode_body`; the §9 masquerade:
//! `ingress_openai_encode_errors`; the §5 stash: `ingress_openai_encode_stash`.

use serde_json::json;

use super::ingress_encode_support::{
    egress_decode, encode_all, json_delta, ms, payloads, sig_delta, state, stop, stream_req,
    text_delta, text_start, think_delta, think_start, tool_start, usage_event,
};
use crate::{ContentKind, Delta, Event, FinishReason, Role};

const TOOLS: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_tools.sse");

fn usage_req() -> serde_json::Value {
    json!({"model": "gpt-4o", "messages": [], "stream": true,
        "stream_options": {"include_usage": true}})
}

#[test]
fn basic_stream_with_usage_is_the_exact_wire() {
    let mut st = state(usage_req(), &[]);
    let events = [
        ms(),
        text_start(0),
        text_delta(0, "Hel"),
        text_delta(0, "lo"),
        stop(0),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        usage_event(),
        Event::End,
    ];
    // The §3.7 trace, inverted: role-only first delta, per-fragment content
    // chunks, the finish chunk, the post-finish usage chunk (include_usage
    // asked), and the sentinel — every chunk carrying the fabricated identity.
    let want = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"role\":\"assistant\"},\"finish_reason\":null,\"index\":0}],\"created\":1700000000,\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion.chunk\",\"usage\":null}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null,\"index\":0}],\"created\":1700000000,\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion.chunk\",\"usage\":null}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null,\"index\":0}],\"created\":1700000000,\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion.chunk\",\"usage\":null}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}],\"created\":1700000000,\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion.chunk\",\"usage\":null}\n\n",
        "data: {\"choices\":[],\"created\":1700000000,\"id\":\"chatcmpl-9\",\"model\":\"gpt-4o-2024-08-06\",\"object\":\"chat.completion.chunk\",\"usage\":{\"completion_tokens\":2,\"prompt_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":0},\"total_tokens\":14}}\n\n",
        "data: [DONE]\n\n",
    );
    assert_eq!(encode_all(&events, &mut st), want);
    assert_eq!(st.status(), 200); // no Error event → the listener answers 200
}

#[test]
fn encode_then_egress_decode_is_identity() {
    // §14's round-trip property on the response side: the EGRESS decoder run
    // over the ingress-encoded SSE reproduces the exact canonical script (the
    // two codecs check each other; the run-owned End is appended like run does).
    let events = vec![
        ms(),
        text_start(0),
        text_delta(0, "Hel"),
        text_delta(0, "lo"),
        tool_start(1, "call_x", "get_weather"),
        json_delta(1, "{\"location\""),
        json_delta(1, ":\"Paris\"}"),
        stop(0),
        stop(1),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        usage_event(),
        Event::End,
    ];
    let mut st = state(usage_req(), &[]);
    let (mut got, terminated) = egress_decode(&encode_all(&events, &mut st));
    got.push(Event::End);
    assert_eq!(got, events);
    assert!(terminated); // finish_reason + [DONE]: a clean terminal stream
}

#[test]
fn tool_call_deltas_match_the_captured_transcript() {
    // The §14 pin against a REAL captured OpenAI SSE transcript: decode the
    // fixture to canonical events, re-encode through ingress, and the
    // tool-bearing delta shapes match the capture chunk for chunk (id +
    // function.name only on a call's first chunk, then index-routed fragments).
    let (mut events, _) = egress_decode(std::str::from_utf8(TOOLS).unwrap());
    events.push(Event::End);
    let mut st = state(stream_req(), &[]);
    let ours = payloads(&encode_all(&events, &mut st));
    let real = payloads(std::str::from_utf8(TOOLS).unwrap());
    assert_eq!(ours.len(), real.len());
    for (o, r) in ours.iter().zip(&real) {
        assert_eq!(
            o["choices"][0]["delta"]["tool_calls"],
            r["choices"][0]["delta"]["tool_calls"]
        );
        assert_eq!(
            o["choices"][0]["finish_reason"],
            r["choices"][0]["finish_reason"]
        );
        assert_eq!(o["id"], r["id"]); // MessageStart carried the upstream id
    }
}

#[test]
fn identity_is_fabricated_when_upstream_names_none() {
    let mut st = state(stream_req(), &[]);
    let events = [Event::message_start(None, None, Role::Assistant)];
    let p = &payloads(&encode_all(&events, &mut st))[0];
    assert_eq!(p["id"], json!("chatcmpl-brazen-1700000000")); // Clock-derived
    assert_eq!(p["model"], json!("gpt-4o")); // the client-requested model
    assert_eq!(p["created"], json!(1_700_000_000));
    // no usage ask → no usage slot on chunks (the real wire omits it too)
    assert!(!p.as_object().unwrap().contains_key("usage"));
}

#[test]
fn finish_vocabulary_maps_from_canonical_finish() {
    let cases = [
        (FinishReason::Stop, "stop"),
        (FinishReason::StopSequence, "stop"), // chat's own spelling of a hit
        (FinishReason::Pause, "stop"),        // no pause vocabulary on this wire
        (FinishReason::Length, "length"),
        (FinishReason::ToolUse, "tool_calls"),
        (FinishReason::Other("bespoke".into()), "bespoke"), // verbatim, never panic
        (
            FinishReason::Refusal {
                category: "content_filter".into(),
                explanation: None,
            },
            "content_filter",
        ),
    ];
    for (reason, want) in cases {
        let mut st = state(stream_req(), &[]);
        let sse = encode_all(&[Event::Finish { reason }], &mut st);
        assert_eq!(
            payloads(&sse)[0]["choices"][0]["finish_reason"],
            json!(want)
        );
    }
}

#[test]
fn refusal_with_text_restreams_the_refusal_channel() {
    // §3.5 inverted: the text-bearing refusal rides its own `delta.refusal`
    // chunk and finishes "stop" — exactly what the forward table decodes back
    // to Refusal{category:"refusal"}.
    let mut st = state(stream_req(), &[]);
    let sse = encode_all(
        &[Event::Finish {
            reason: FinishReason::Refusal {
                category: "refusal".into(),
                explanation: Some("cannot help".into()),
            },
        }],
        &mut st,
    );
    let p = payloads(&sse);
    assert_eq!(
        p[0]["choices"][0]["delta"],
        json!({"refusal": "cannot help"})
    );
    assert_eq!(p[0]["choices"][0]["finish_reason"], json!(null));
    assert_eq!(p[1]["choices"][0]["delta"], json!({}));
    assert_eq!(p[1]["choices"][0]["finish_reason"], json!("stop"));
}

#[test]
fn adaptation_comments_precede_the_first_chunk() {
    // §4 runtime exposure, stream shape: SSE comment lines — spec-legal,
    // invisible to conforming parsers — ride just before the first frame.
    let mut st = state(stream_req(), &["thinking_replay", "document_url_drop"]);
    let sse = encode_all(&[ms(), Event::End], &mut st);
    assert!(sse.starts_with(
        ": brazen adaptation=thinking_replay\n: brazen adaptation=document_url_drop\ndata: "
    ));
    let comments: Vec<&str> = sse.lines().filter(|l| l.starts_with(':')).collect();
    assert_eq!(comments.len(), 2); // exposed once, not re-flushed per chunk
}

#[test]
fn adaptation_comments_survive_a_chunkless_stream() {
    // No chunk ever emitted (an empty turn): the sentinel carries the comments.
    let mut st = state(stream_req(), &["thinking_replay"]);
    assert_eq!(
        encode_all(&[Event::End], &mut st),
        ": brazen adaptation=thinking_replay\ndata: [DONE]\n\n"
    );
}

#[test]
fn dialectless_events_emit_no_client_bytes() {
    // Reasoning and server-tool blocks have no slot on this wire (they route to
    // the §5 stash); --raw-only / forward-compat events carry no client fact.
    let mut st = state(stream_req(), &[]);
    assert!(!encode_all(&[ms()], &mut st).is_empty());
    let silent = [
        text_start(0),
        Event::ContentDelta {
            index: 9, // a delta on an index no start ever opened: dropped
            delta: Delta::TextDelta("lost".into()),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::Other(json!({"future": true})),
        },
        think_start(1, None),
        think_delta(1, "hmm"),
        sig_delta(1, "sig"),
        stop(1),
        Event::ContentStart {
            index: 2,
            kind: ContentKind::RedactedThinking {
                data: "blob".into(),
            },
        },
        Event::ContentStart {
            index: 3,
            kind: ContentKind::ServerToolUse {
                id: "srv_1".into(),
                name: "web_search".into(),
            },
        },
        stop(3),
        stop(0),
        stop(7), // a stop for an index never opened: dropped
        Event::Raw(vec![0xFF]),
        Event::Other,
        usage_event(), // include_usage NOT asked → the usage chunk is withheld
    ];
    for e in &silent {
        assert_eq!(encode_all(std::slice::from_ref(e), &mut st), "", "{e:?}");
    }
}
