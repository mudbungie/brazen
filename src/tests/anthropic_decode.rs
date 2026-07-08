//! Fine-grained `decode` branch coverage (anthropic-messages §3/§4): each
//! `data.type`, each delta kind, and the malformed/absent-field guards — crafted
//! frames, no network. The terminal aspect (every `stop_reason`, the error-kind
//! table, the non-JSON-body guard) lives in `anthropic_decode_finish`.

use crate::protocol::anthropic::AnthropicMessages;
use crate::{ContentKind, DecodeState, Delta, Event, Frame, Protocol, Role, Usage};
use serde_json::{json, Value};

/// Decode one streamed frame (a normal SSE block payload) against `state`.
fn dec(v: Value, state: &mut DecodeState) -> Vec<Event> {
    let data = serde_json::to_vec(&v).unwrap();
    let frame = Frame {
        event: None,
        data,
        status: None,
    };
    AnthropicMessages.decode(frame, state).unwrap()
}

#[test]
fn message_start_without_usage_is_just_message_start() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"message_start","message":{"id":"m","model":"x","role":"assistant"}}),
        &mut s,
    );
    assert_eq!(
        ev,
        vec![Event::message_start(
            Some("m".into()),
            Some("x".into()),
            Role::Assistant
        )]
    );
}

#[test]
fn ping_and_unknown_types_emit_nothing() {
    let mut s = DecodeState::default();
    assert_eq!(dec(json!({"type":"ping"}), &mut s), vec![]);
    assert_eq!(dec(json!({"type":"future_event"}), &mut s), vec![]);
    assert!(!s.terminated);
}

#[test]
fn message_stop_sets_terminated_and_emits_nothing() {
    let mut s = DecodeState::default();
    assert_eq!(dec(json!({"type":"message_stop"}), &mut s), vec![]);
    assert!(s.terminated);
}

#[test]
fn redacted_thinking_block_opens_with_no_delta() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"content_block_start","index":0,
               "content_block":{"type":"redacted_thinking","data":"OPAQUE=="}}),
        &mut s,
    );
    let kind = ContentKind::RedactedThinking {
        data: "OPAQUE==".into(),
    };
    // Opens (and is tracked) carrying the opaque `data` INLINE at start; no delta (bl-61a9).
    assert_eq!(
        ev,
        vec![Event::ContentStart {
            index: 0,
            kind: kind.clone()
        }]
    );
    assert_eq!(s.open.get(&0).map(|b| b.kind.clone()), Some(kind));
}

#[test]
fn tool_use_json_fragments_emit_directly_as_deltas() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":1,
               "content_block":{"type":"tool_use","id":"tu","name":"f","input":{}}}),
        &mut s,
    );
    let d1 = dec(
        json!({"type":"content_block_delta","index":1,
               "delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}),
        &mut s,
    );
    // The fragment surfaces verbatim as a JsonDelta — emitted directly, never
    // buffered/parsed mid-stream.
    assert_eq!(
        d1,
        vec![Event::ContentDelta {
            index: 1,
            delta: Delta::JsonDelta("{\"a\":1}".into())
        }]
    );
}

#[test]
fn signature_delta_emits_a_signature_delta() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":0,
               "content_block":{"type":"thinking","thinking":"","signature":""}}),
        &mut s,
    );
    let sig = dec(
        json!({"type":"content_block_delta","index":0,
               "delta":{"type":"signature_delta","signature":"SIG=="}}),
        &mut s,
    );
    // bl-61a9 (CR-5 resolved): surfaces as a SignatureDelta a sink folds onto the signature.
    assert_eq!(
        sig,
        vec![Event::ContentDelta {
            index: 0,
            delta: Delta::SignatureDelta("SIG==".into()),
        }]
    );
    assert!(s.open.contains_key(&0)); // block stays tracked so its terminal stop fires
}

#[test]
fn unknown_delta_on_a_tracked_block_emits_nothing() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        &mut s,
    );
    assert_eq!(
        dec(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"bogus_delta"}}),
            &mut s
        ),
        vec![]
    );
}

#[test]
fn untracked_block_deltas_and_stops_emit_nothing() {
    let mut s = DecodeState::default();
    // A block kind with no canonical ContentKind (server_tool_use now HAS one —
    // CR-4 resolved — so this uses a genuinely unknown tag): start is dropped, the
    // index never enters `open`, and its delta/stop fall through to [].
    assert_eq!(
        dec(
            json!({"type":"content_block_start","index":3,
                   "content_block":{"type":"wavelet_block","data":"x"}}),
            &mut s
        ),
        vec![]
    );
    assert_eq!(
        dec(
            json!({"type":"content_block_delta","index":3,
                   "delta":{"type":"input_json_delta","partial_json":"x"}}),
            &mut s
        ),
        vec![]
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop","index":3}), &mut s),
        vec![]
    );
}

#[test]
fn absent_index_defaults_to_zero() {
    let mut s = DecodeState::default();
    dec(
        json!({"type":"content_block_start","content_block":{"type":"text","text":""}}),
        &mut s,
    );
    assert_eq!(
        dec(json!({"type":"content_block_stop"}), &mut s),
        vec![Event::ContentStop { index: 0 }]
    );
}

#[test]
fn message_delta_usage_only_emits_no_finish() {
    let mut s = DecodeState::default();
    let ev = dec(
        json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":7}}),
        &mut s,
    );
    assert_eq!(
        ev,
        vec![Event::Usage(Usage {
            output_tokens: Some(7),
            ..Usage::default()
        })]
    );
}
