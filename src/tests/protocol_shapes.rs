//! `Protocol::shapes()` is a CLAIM about a dialect's own `encode` (config §6.1), and
//! this is the proof — the sibling of `protocol_tuning.rs` one level up. `tuning` asks
//! "does the knob reach the wire?" and proves it by DIFFING two encodes; `shapes` asks
//! "does the request's shape reach the wire at all?", and the only thing that can mean
//! is **the shape encodes iff the dialect declares it**. A dialect that grows a
//! projection and forgets the declaration fails here, and so does the reverse.
//!
//! Key-agnostic on purpose, exactly as its sibling is: no wire spelling is restated, so
//! there is no second table to drift from the encoders.

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::claude_code::ClaudeCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{CanonicalRequest, Protocol, ProviderCtx, WireRequest};
use serde_json::json;

/// Every shipped dialect, as `protocol_tuning.rs` lists them. A new protocol variant is
/// one line here and it is under this proof.
fn dialects() -> Vec<&'static dyn Protocol> {
    vec![
        &OpenAiChat,
        &OpenAiResponses,
        &AnthropicMessages,
        &GoogleGenAi,
        &OllamaChat,
        &ClaudeCode,
    ]
}

/// The simplest request EVERY dialect can encode: one user text turn plus the token cap
/// Anthropic requires. Both shape cases below are this request with ONE thing added.
fn base() -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 64,
    }))
    .unwrap()
}

/// `base()` plus a tool declaration — the shape `shapes().tools` claims.
fn with_tools() -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 64,
        "tools": [{
            "name": "now",
            "description": "the time",
            "input_schema": {"type": "object", "properties": {}},
        }],
    }))
    .unwrap()
}

/// `base()` plus an assistant turn and a second user turn — the shape
/// `shapes().multi_turn` claims. Text-only, so a dialect that refuses this refuses the
/// TRANSCRIPT and not the content kind.
fn with_transcript() -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
            {"role": "user", "content": "again"},
        ],
        "max_tokens": 64,
    }))
    .unwrap()
}

fn enc(proto: &dyn Protocol, req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let ctx = ProviderCtx {
        base_url: "https://host",
        model: "m",
        beta_headers: &[],
        // Only the exec dialect reads it; the HTTP dialects ignore it entirely.
        exec: Some("claude"),
    };
    proto.encode(req, &ctx)
}

/// Encodes, or refuses the way arch §3.1 requires: `ParseInput` → 64, never a silent
/// drop. Asserting the KIND is half the proof — a dialect that "declines" by encoding
/// the request without the tools would pass a bare `is_ok` check while lying to every
/// caller.
fn carries(proto: &dyn Protocol, req: &CanonicalRequest) -> bool {
    match enc(proto, req) {
        Ok(_) => true,
        Err(e) => {
            assert_eq!(
                e.kind,
                ErrorKind::ParseInput,
                "an unrepresentable shape must reject as ParseInput (arch §3.1)"
            );
            false
        }
    }
}

/// `shapes().tools` ⇔ this dialect's own `encode` accepts a tool-bearing request.
#[test]
fn every_dialect_carries_tools_exactly_as_it_declares() {
    for proto in dialects() {
        assert!(carries(proto, &base()), "the base request must encode");
        assert_eq!(
            proto.shapes().tools,
            carries(proto, &with_tools()),
            "a dialect's `shapes().tools` disagrees with its own encode"
        );
    }
}

/// `shapes().multi_turn` ⇔ this dialect's own `encode` accepts a replayed transcript.
#[test]
fn every_dialect_carries_a_transcript_exactly_as_it_declares() {
    for proto in dialects() {
        assert_eq!(
            proto.shapes().multi_turn,
            carries(proto, &with_transcript()),
            "a dialect's `shapes().multi_turn` disagrees with its own encode"
        );
    }
}

/// The declaration is DATA — cheap, pure, identical on every call, so the read surface
/// asks it per row without a cache (and `Shapes`' derives are exercised).
#[test]
fn the_declaration_is_a_stable_value() {
    for proto in dialects() {
        assert_eq!(proto.shapes(), proto.shapes());
        assert!(!format!("{:?}", proto.shapes()).is_empty());
    }
    // The asymmetry the listing exists to publish: exactly one shipped dialect carries
    // neither shape, and it is the one whose refusal cost a debugging session (bl-68ad).
    let (yes, no): (Vec<_>, Vec<_>) = dialects().into_iter().partition(|p| p.shapes().tools);
    assert_eq!(yes.len(), 5);
    assert_eq!(no.len(), 1);
    assert!(!ClaudeCode.shapes().tools);
    assert!(!ClaudeCode.shapes().multi_turn);
    assert!(OpenAiChat.shapes().tools);
    assert!(OpenAiChat.shapes().multi_turn);
}
