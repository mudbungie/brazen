//! Event taxonomy tests (§3.2, §5.2): the wire bytes match the documented
//! NDJSON sample, every variant round-trips, and `Raw` refuses serialization.
//! The `v=1` forward-compat contract (unknown type/kind/delta/reason → `Other`,
//! malformed bytes → `Err`) lives in `canonical_event_compat`.

use crate::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, Role, Usage,
    EVENT_SCHEMA_VERSION,
};

fn rt(ev: &Event) -> Event {
    let s = serde_json::to_string(ev).unwrap();
    serde_json::from_str(&s).unwrap()
}

#[test]
fn message_start_stamps_the_schema_version() {
    let ev = Event::message_start(Some("msg_1".into()), Some("m".into()), Role::Assistant);
    match ev {
        Event::MessageStart { v, .. } => assert_eq!(v, EVENT_SCHEMA_VERSION),
        other => panic!("expected MessageStart, got {other:?}"),
    }
    assert_eq!(EVENT_SCHEMA_VERSION, 1);
}

#[test]
fn wire_bytes_match_the_5_2_sample() {
    let lines = [
        (
            Event::message_start(
                Some("msg_01…".into()),
                Some("claude-3-5-sonnet".into()),
                Role::Assistant,
            ),
            r#"{"type":"message_start","v":1,"id":"msg_01…","model":"claude-3-5-sonnet","role":"assistant"}"#,
        ),
        (
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {},
            },
            r#"{"type":"content_start","index":0,"kind":{"text":{}}}"#,
        ),
        (
            Event::ContentDelta {
                index: 0,
                delta: Delta::TextDelta("Hel".into()),
            },
            r#"{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}"#,
        ),
        (
            Event::ContentStop { index: 0 },
            r#"{"type":"content_stop","index":0}"#,
        ),
        (
            Event::Usage(Usage {
                input_tokens: Some(12),
                output_tokens: Some(2),
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            r#"{"type":"usage","input_tokens":12,"output_tokens":2,"cache_read_tokens":null,"cache_write_tokens":null}"#,
        ),
        (
            Event::Finish {
                reason: FinishReason::Stop,
            },
            r#"{"type":"finish","reason":"stop"}"#,
        ),
        (Event::End, r#"{"type":"end"}"#),
    ];
    for (ev, wire) in lines {
        assert_eq!(serde_json::to_string(&ev).unwrap(), wire, "wire for {ev:?}");
        assert_eq!(rt(&ev), ev, "round-trip {ev:?}");
    }
}

#[test]
fn content_kind_and_delta_variants_roundtrip() {
    let evs = [
        Event::ContentStart {
            index: 1,
            kind: ContentKind::ToolUse {
                id: "t1".into(),
                name: "search".into(),
            },
        },
        Event::ContentStart {
            index: 2,
            kind: ContentKind::Thinking {},
        },
        Event::ContentStart {
            index: 3,
            kind: ContentKind::RedactedThinking {},
        },
        Event::ContentDelta {
            index: 1,
            delta: Delta::JsonDelta("{\"q\":".into()),
        },
        Event::ContentDelta {
            index: 2,
            delta: Delta::ThinkingDelta("hmm".into()),
        },
    ];
    for ev in evs {
        assert_eq!(rt(&ev), ev, "round-trip {ev:?}");
    }
}

#[test]
fn server_tool_kinds_pin_wire_bytes_and_roundtrip() {
    // The fixed server_tool_use tag and the DYNAMIC result tag both render in the
    // externally-tagged position: `"kind":{<tag>:{…}}` — the result's tag IS its
    // `kind`, re-emitted verbatim (the open-set rule applied to result blocks).
    let lines = [
        (
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ServerToolUse {
                    id: "srvtoolu_1".into(),
                    name: "web_search".into(),
                },
            },
            r#"{"type":"content_start","index":1,"kind":{"server_tool_use":{"id":"srvtoolu_1","name":"web_search"}}}"#,
        ),
        (
            Event::ContentStart {
                index: 2,
                kind: ContentKind::ServerToolResult {
                    kind: "web_search_tool_result".into(),
                    tool_use_id: "srvtoolu_1".into(),
                    content: serde_json::json!([{"type": "web_search_result", "url": "https://x"}]),
                },
            },
            r#"{"type":"content_start","index":2,"kind":{"web_search_tool_result":{"tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://x"}]}}}"#,
        ),
    ];
    for (ev, wire) in lines {
        assert_eq!(serde_json::to_string(&ev).unwrap(), wire, "wire for {ev:?}");
        assert_eq!(rt(&ev), ev, "round-trip {ev:?}");
    }
    // The suffix arm generalizes: a result tag brazen never enumerated (and an
    // error-object content Value) round-trips with zero per-tool knowledge.
    let code = Event::ContentStart {
        index: 3,
        kind: ContentKind::ServerToolResult {
            kind: "code_execution_tool_result".into(),
            tool_use_id: "srvtoolu_2".into(),
            content: serde_json::json!({"type": "code_execution_tool_result_error", "error_code": "unavailable"}),
        },
    };
    assert_eq!(rt(&code), code);
}

#[test]
fn error_event_roundtrips() {
    let ev = Event::Error(CanonicalError {
        kind: ErrorKind::Provider { status: 529 },
        message: "overloaded".into(),
        provider_detail: None,
        retry_after_seconds: None,
    });
    assert_eq!(rt(&ev), ev);
}

#[test]
fn raw_refuses_serialization() {
    // `Raw` is `serde(skip)` — written verbatim by the raw sink, never serde.
    assert!(serde_json::to_string(&Event::Raw(vec![1, 2, 3])).is_err());
}

#[test]
fn finish_reason_every_variant_roundtrips() {
    let reasons = [
        FinishReason::Stop,
        FinishReason::Length,
        FinishReason::ToolUse,
        FinishReason::StopSequence,
        FinishReason::Pause,
        FinishReason::Refusal {
            category: "policy".into(),
            explanation: Some("disallowed".into()),
        },
        FinishReason::Refusal {
            category: "policy".into(),
            explanation: None,
        },
        FinishReason::Other("supernova".into()),
    ];
    for reason in reasons {
        let ev = Event::Finish {
            reason: reason.clone(),
        };
        assert_eq!(rt(&ev), ev, "round-trip {reason:?}");
    }
}

#[test]
fn usage_defaults_to_all_unknown() {
    assert_eq!(
        Usage::default(),
        Usage {
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    );
}
