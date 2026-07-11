//! Shared harness for the `anthropic_messages` ingress ENCODE suites (ingress.md
//! §14): state built through the production door (a decoded client request + the
//! fired-adaptations list + a `FakeClock`), events encoded one at a time, bytes
//! concatenated; plus the egress-decode bridge for the codecs-check-each-other
//! property. The generic event builders (`ms`, `text_start`, …) are reused from the
//! sibling `ingress_encode_support`. `#![allow(dead_code)]` — each suite uses a subset.
#![allow(dead_code)]

use serde_json::{json, Value};

use crate::protocol::anthropic::AnthropicMessages;
use crate::testing::FakeClock;
use crate::{
    decode_request, encode_response, DecodeState, Event, Framing, IngressId, IngressState,
    Protocol, Role,
};

/// The upstream identity every anthropic golden pins (a native `msg_` id).
pub fn ms() -> Event {
    Event::message_start(
        Some("msg_01XYZ".into()),
        Some("claude-opus-4-8".into()),
        Role::Assistant,
    )
}

/// The FakeClock instant every golden pins `created` to.
pub const CREATED: u64 = 1_700_000_000;

/// State for one response, built the way production builds it: the decoded client
/// request supplies the shape knobs, the caller supplies the fired adaptations.
pub fn state(req: Value, adaptations: &[&str]) -> IngressState {
    let req = decode_request(IngressId::AnthropicMessages, req.to_string().as_bytes()).unwrap();
    IngressState::for_request(
        &req,
        adaptations.iter().map(|s| (*s).to_owned()).collect(),
        &FakeClock::new(CREATED),
    )
}

/// A plain `stream:true` request.
pub fn stream_req() -> Value {
    json!({"model": "claude-x", "messages": [], "stream": true, "max_tokens": 16})
}

/// An aggregate request (`stream` absent — the default shape, ingress.md §10).
pub fn agg_req() -> Value {
    json!({"model": "claude-x", "messages": [], "max_tokens": 16})
}

/// Encode `events` through the dialect dispatch, concatenating every chunk.
pub fn encode_all(events: &[Event], state: &mut IngressState) -> String {
    let mut out = Vec::new();
    for e in events {
        out.extend(encode_response(IngressId::AnthropicMessages, e, state));
    }
    String::from_utf8(out).unwrap()
}

/// Run the EGRESS `anthropic_messages` decoder over ingress-encoded SSE bytes — §14's
/// codecs-check-each-other bridge. Returns the events and `terminated`.
pub fn egress_decode(sse: &str) -> (Vec<Event>, bool) {
    let mut dec = Framing::Sse.decoder();
    let mut frames = dec.push(sse.as_bytes().to_vec()).unwrap();
    frames.extend(dec.finish().unwrap());
    let mut ds = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(AnthropicMessages.decode(f, &mut ds).unwrap());
    }
    (events, ds.terminated)
}

/// Parse every `data:` payload of an anthropic SSE byte string (skipping the
/// `event:`/comment lines) — the shape-level view an SDK parser sees.
pub fn payloads(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .map(|p| serde_json::from_str(p).unwrap())
        .collect()
}
