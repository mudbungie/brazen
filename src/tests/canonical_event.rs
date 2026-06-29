//! Event taxonomy tests (§3.2, §5.2): the wire bytes match the documented
//! NDJSON sample, every variant round-trips, `Raw` refuses serialization, and
//! the `v=1` forward-compat contract holds — an unknown event `type`, content
//! `kind`, or `delta` decodes to `Other` (the general path) instead of erroring,
//! exactly as `FinishReason::Other` already preserves an unknown reason.

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
fn error_event_roundtrips() {
    let ev = Event::Error(CanonicalError {
        kind: ErrorKind::Provider { status: 529 },
        message: "overloaded".into(),
        provider_detail: None,
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
fn finish_reason_unknown_value_is_other_not_a_panic() {
    let ev: Event = serde_json::from_str(r#"{"type":"finish","reason":"time_warp"}"#).unwrap();
    assert_eq!(
        ev,
        Event::Finish {
            reason: FinishReason::Other("time_warp".into()),
        }
    );
}

#[test]
fn malformed_bytes_surface_a_deserialize_error_not_a_panic() {
    // ContentKind/Delta/FinishReason are part of the typed interface (§9.8), so an
    // embedder can deserialize them directly. Their hand-rolled `Deserialize` opens
    // with a fallible `Value::deserialize(d)?` / `Raw::deserialize(d)?`; truncated
    // JSON must surface that Err (the genuinely-reachable path), never a panic.
    assert!(serde_json::from_str::<ContentKind>("{").is_err());
    assert!(serde_json::from_str::<Delta>("{").is_err());
    assert!(serde_json::from_str::<FinishReason>("{").is_err());
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

#[test]
fn unknown_event_type_decodes_to_other_not_an_error() {
    // The `v=1` forward-compat contract (§3.2): a future additive event `type`
    // a pinned build doesn't model decodes to `Event::Other` (the skip path)
    // instead of erroring. `#[serde(other)]` drops the payload.
    let ev: Event =
        serde_json::from_str(r#"{"type":"reasoning_summary","index":0,"text":"…"}"#).unwrap();
    assert_eq!(ev, Event::Other);
    assert_eq!(
        serde_json::to_string(&Event::Other).unwrap(),
        r#"{"type":"other"}"#
    );
    assert_eq!(rt(&Event::Other), Event::Other);
}

#[test]
fn unknown_content_kind_rides_other_verbatim() {
    // A future `ContentKind` (e.g. web-search results — the §3.2 deferred kinds):
    // decodes to `Other` carrying the whole `{tag: body}` object, lossless for
    // passthrough, instead of erroring.
    let wire = r#"{"type":"content_start","index":7,"kind":{"web_search_result":{"url":"x"}}}"#;
    let ev: Event = serde_json::from_str(wire).unwrap();
    assert_eq!(
        ev,
        Event::ContentStart {
            index: 7,
            kind: ContentKind::Other(serde_json::json!({"web_search_result": {"url": "x"}})),
        }
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), wire);
    assert_eq!(rt(&ev), ev);
    // Tolerant even of a non-object `kind` shape (the catch-all is the general
    // path, not just "unknown object tag").
    let scalar: Event =
        serde_json::from_str(r#"{"type":"content_start","index":0,"kind":"surprise"}"#).unwrap();
    assert_eq!(
        scalar,
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Other(serde_json::json!("surprise")),
        }
    );
}

#[test]
fn unknown_delta_rides_other_verbatim() {
    let wire = r#"{"type":"content_delta","index":2,"delta":{"citation_delta":{"n":1}}}"#;
    let ev: Event = serde_json::from_str(wire).unwrap();
    assert_eq!(
        ev,
        Event::ContentDelta {
            index: 2,
            delta: Delta::Other(serde_json::json!({"citation_delta": {"n": 1}})),
        }
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), wire);
    assert_eq!(rt(&ev), ev);
}

#[test]
fn unknown_object_fields_on_a_known_event_are_ignored() {
    // The other half of the contract: a new additive FIELD on a known event is
    // dropped, never a parse error (serde ignores unknown fields by default).
    let ev: Event =
        serde_json::from_str(r#"{"type":"content_stop","index":0,"future_field":true}"#).unwrap();
    assert_eq!(ev, Event::ContentStop { index: 0 });
}
