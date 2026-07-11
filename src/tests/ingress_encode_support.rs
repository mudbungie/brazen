//! Shared harness for the `openai_chat` ingress ENCODE suites (ingress.md §14):
//! state built through the production door (a decoded client request + the
//! fired-adaptations list + a `FakeClock`), events encoded one at a time, bytes
//! concatenated; plus the egress-decode bridge for the codecs-check-each-other
//! property. `#![allow(dead_code)]` because each suite uses only a subset.
#![allow(dead_code)]

use serde_json::{json, Value};

use crate::protocol::openai::OpenAiChat;
use crate::testing::FakeClock;
use crate::{
    decode_request, encode_response, ContentKind, DecodeState, Delta, Event, Framing, IngressId,
    IngressState, Protocol, Role, Usage,
};

/// The FakeClock instant every golden pins `created` to.
pub const CREATED: u64 = 1_700_000_000;

/// State for one response, built the way production builds it: the decoded
/// client request supplies the shape knobs (`stream`; `stream_options` rides
/// `extra`), the caller supplies the fired adaptations, the clock is fake.
pub fn state(req: Value, adaptations: &[&str]) -> IngressState {
    let req = decode_request(IngressId::OpenAiChat, req.to_string().as_bytes()).unwrap();
    IngressState::for_request(
        &req,
        adaptations.iter().map(|s| (*s).to_owned()).collect(),
        &FakeClock::new(CREATED),
    )
}

/// A plain `stream:true` request (no usage ask).
pub fn stream_req() -> Value {
    json!({"model": "gpt-4o", "messages": [], "stream": true})
}

/// An aggregate request (`stream` absent — the default shape, ingress.md §10).
pub fn agg_req() -> Value {
    json!({"model": "gpt-4o", "messages": []})
}

/// Encode `events` through the dialect dispatch, concatenating every chunk.
pub fn encode_all(events: &[Event], state: &mut IngressState) -> String {
    let mut out = Vec::new();
    for e in events {
        out.extend(encode_response(IngressId::OpenAiChat, e, state));
    }
    String::from_utf8(out).unwrap()
}

/// The upstream identity every golden pins.
pub fn ms() -> Event {
    Event::message_start(
        Some("chatcmpl-9".into()),
        Some("gpt-4o-2024-08-06".into()),
        Role::Assistant,
    )
}

pub fn text_start(index: u32) -> Event {
    Event::ContentStart {
        index,
        kind: ContentKind::Text {},
    }
}

pub fn text_delta(index: u32, s: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::TextDelta(s.into()),
    }
}

pub fn tool_start(index: u32, id: &str, name: &str) -> Event {
    Event::ContentStart {
        index,
        kind: ContentKind::ToolUse {
            id: id.into(),
            name: name.into(),
        },
    }
}

pub fn json_delta(index: u32, s: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::JsonDelta(s.into()),
    }
}

pub fn think_start(index: u32, id: Option<&str>) -> Event {
    Event::ContentStart {
        index,
        kind: ContentKind::Thinking {
            id: id.map(str::to_owned),
        },
    }
}

pub fn think_delta(index: u32, s: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::ThinkingDelta(s.into()),
    }
}

pub fn sig_delta(index: u32, s: &str) -> Event {
    Event::ContentDelta {
        index,
        delta: Delta::SignatureDelta(s.into()),
    }
}

pub fn stop(index: u32) -> Event {
    Event::ContentStop { index }
}

/// The §14 usage fixture: 12 in, 2 out, an explicitly-zero cache read.
pub fn usage_event() -> Event {
    Event::Usage(Usage {
        input_tokens: Some(12),
        output_tokens: Some(2),
        cache_read_tokens: Some(0),
        cache_write_tokens: None,
    })
}

/// Parse every `data:` payload of an SSE byte string (skipping comment lines
/// and the `[DONE]` sentinel) — the shape-level view an SDK parser sees.
pub fn payloads(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|p| *p != "[DONE]")
        .map(|p| serde_json::from_str(p).unwrap())
        .collect()
}

/// Run the EGRESS `openai_chat` decoder over ingress-encoded SSE bytes — §14's
/// codecs-check-each-other bridge. Returns the events and `terminated`.
pub fn egress_decode(sse: &str) -> (Vec<Event>, bool) {
    let mut dec = Framing::Sse.decoder();
    let mut frames = dec.push(sse.as_bytes().to_vec()).unwrap();
    frames.extend(dec.finish().unwrap());
    let mut ds = DecodeState::default();
    let mut events = Vec::new();
    for f in frames {
        events.extend(OpenAiChat.decode(f, &mut ds).unwrap());
    }
    (events, ds.terminated)
}
