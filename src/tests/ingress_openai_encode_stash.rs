//! The §5 stash-write join point (ingress.md, tested per §14): when a canonical
//! event carries an opaque replay payload — `Thinking`
//! `signature`/`encrypted_content`/item id, `RedactedThinking` data, `ToolUse`
//! signature — the encoder surfaces `(key, canonical-JSON payload)` pairs at
//! `End`: every tool-call id for a tool-bearing turn (any echoed id joins on
//! replay), else the shared `content_key` hash of the turn's text. The encoder
//! only EMITS pairs — the listener writes the stash.

use serde_json::{json, Value};

use super::ingress_encode_support::{
    agg_req, encode_all, json_delta, ms, sig_delta, state, stop, stream_req, text_delta,
    text_start, think_delta, think_start, tool_start,
};
use crate::store::content_key;
use crate::{ContentKind, Delta, Event, FinishReason};

fn parsed(payload: &[u8]) -> Value {
    serde_json::from_slice(payload).unwrap()
}

#[test]
fn a_signed_thinking_turn_keys_on_every_tool_call_id() {
    // The Anthropic shape: a signed thinking block before tool calls — exactly
    // the turn whose replay REQUIRES the block (§5). Both ids carry the same
    // payload, so whichever id the client echoes recalls it.
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        think_start(0, None),
        think_delta(0, "let me think"),
        sig_delta(0, "sig"),
        sig_delta(0, "1"), // fragments concatenate, like every opaque delta
        stop(0),
        tool_start(1, "call_a", "get_weather"),
        json_delta(1, "{}"),
        stop(1),
        tool_start(2, "call_b", "get_time"),
        stop(2),
        Event::Finish {
            reason: FinishReason::ToolUse,
        },
        Event::End,
    ];
    encode_all(&events, &mut st);
    let pairs = st.take_stash();
    let want = json!([{"signature": "sig1", "text": "let me think", "type": "thinking"}]);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "call_a");
    assert_eq!(pairs[1].0, "call_b");
    assert_eq!(parsed(&pairs[0].1), want);
    assert_eq!(parsed(&pairs[1].1), want);
    assert!(st.take_stash().is_empty()); // the join point drains
}

#[test]
fn a_google_tool_signature_stashes_the_tool_use_block() {
    // The Google thoughtSignature rides the tool call itself: the stash block
    // is the ToolUse with its input parsed only at block close (§5).
    let mut st = state(agg_req(), &[]);
    let events = [
        ms(),
        tool_start(0, "call_g", "lookup"),
        json_delta(0, "{\"x\":"),
        json_delta(0, "1}"),
        sig_delta(0, "tsig"),
        stop(0),
        Event::End,
    ];
    encode_all(&events, &mut st);
    let pairs = st.take_stash();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, "call_g");
    assert_eq!(
        parsed(&pairs[0].1),
        json!([{"id": "call_g", "input": {"x": 1}, "name": "lookup",
            "signature": "tsig", "type": "tool_use"}])
    );
}

#[test]
fn tool_input_shapes_degrade_never_panic() {
    // "" arguments is the wire's empty-input convention → {}; fragments that
    // never became JSON degrade the stash block's input to null (arch §9.5).
    let mut st = state(agg_req(), &[]);
    let events = [
        tool_start(0, "call_e", "noargs"),
        sig_delta(0, "s0"),
        stop(0),
        tool_start(1, "call_m", "mangled"),
        json_delta(1, "not json"),
        sig_delta(1, "s1"),
        stop(1),
        Event::End,
    ];
    encode_all(&events, &mut st);
    let pairs = st.take_stash();
    let blocks = parsed(&pairs[0].1);
    assert_eq!(blocks[0]["input"], json!({}));
    assert_eq!(blocks[1]["input"], json!(null));
}

#[test]
fn a_toolless_turn_keys_on_the_shared_content_hash() {
    // The OpenAI Responses shape: reasoning item id + encrypted_content, no
    // tool calls — the key is content_key(text), the ONE derivation the
    // decoder joins on (src/store/replay.rs).
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        think_start(0, Some("rs_1")),
        think_delta(0, "why"),
        Event::ContentDelta {
            index: 0,
            delta: Delta::EncryptedReasoningDelta("enc".into()),
        },
        stop(0),
        text_start(1),
        text_delta(1, "Paris."),
        stop(1),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ];
    encode_all(&events, &mut st);
    let pairs = st.take_stash();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, content_key("Paris."));
    assert_eq!(
        parsed(&pairs[0].1),
        json!([{"encrypted_content": "enc", "id": "rs_1",
            "signature": null, "text": "why", "type": "thinking"}])
    );
}

#[test]
fn redacted_thinking_stashes_its_opaque_data() {
    let mut st = state(agg_req(), &[]);
    let events = [
        ms(),
        Event::ContentStart {
            index: 0,
            kind: ContentKind::RedactedThinking {
                data: "blob".into(),
            },
        },
        stop(0),
        Event::End,
    ];
    encode_all(&events, &mut st);
    let pairs = st.take_stash();
    assert_eq!(pairs[0].0, content_key("")); // no text, no tools: the empty hash
    assert_eq!(
        parsed(&pairs[0].1),
        json!([{"data": "blob", "type": "redacted_thinking"}])
    );
}

#[test]
fn a_plain_turn_emits_no_pairs() {
    // Unsigned thinking (the Google stream shape) and ordinary text carry no
    // opaque replay payload — nothing to stash, the empty set, not a stub.
    let mut st = state(stream_req(), &[]);
    let events = [
        ms(),
        think_start(0, None),
        think_delta(0, "plain"),
        stop(0),
        text_start(1),
        text_delta(1, "hi"),
        stop(1),
        tool_start(2, "call_p", "f"), // a tool WITHOUT a signature stashes nothing
        stop(2),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ];
    encode_all(&events, &mut st);
    assert!(st.take_stash().is_empty());
}
