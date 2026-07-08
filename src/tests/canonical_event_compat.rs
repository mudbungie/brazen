//! The `v=1` forward-compat contract (§3.2): an unknown event `type`, content
//! `kind`, `delta`, or finish `reason` decodes to `Other` (the general path)
//! instead of erroring; unknown fields on a known event are ignored; malformed
//! bytes surface a deserialize `Err`, never a panic. The known-variant wire
//! bytes and round-trips live in `canonical_event`.

use crate::{ContentKind, Delta, Event, FinishReason};

fn rt(ev: &Event) -> Event {
    let s = serde_json::to_string(ev).unwrap();
    serde_json::from_str(&s).unwrap()
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
