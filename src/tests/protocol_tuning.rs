//! `Protocol::tuning()` is a CLAIM about a dialect's own `encode` (config §6.1), and
//! this is the proof — the cross-check that keeps the read surface honest. It is
//! key-agnostic on purpose: rather than restating each dialect's wire spelling (a
//! second table, free to drift from the encoders and from providers.md §6/§6.2), it
//! asserts the only thing the flag can mean — **setting the knob changes the encoded
//! request iff the dialect declares it projected**. A dialect that grows a projection
//! and forgets the declaration fails here, and so does the reverse.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::claude_code::ClaudeCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{CanonicalRequest, Protocol, ProviderCtx, ReasoningEffort, ServiceTier, WireRequest};
use serde_json::json;

/// Every shipped dialect, reached as the registry hands them out. A new protocol
/// variant is a compile-time addition to `Registry::protocol`; adding it here is the
/// one line that puts it under this proof.
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

/// The simplest request EVERY dialect can encode: one user text turn, a token cap
/// (Anthropic requires it), no tools (claude_code refuses them).
fn base() -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 64,
    }))
    .unwrap()
}

fn enc(proto: &dyn Protocol, req: &CanonicalRequest) -> WireRequest {
    let ctx = ProviderCtx {
        base_url: "https://host",
        model: "m",
        beta_headers: &[],
        // Only the exec dialect reads it; the HTTP dialects ignore it entirely.
        exec: Some("claude"),
    };
    proto.encode(req, &ctx).unwrap()
}

/// `tuning().effort` ⇔ setting `req.reasoning` changes what this dialect encodes.
#[test]
fn every_dialect_projects_the_effort_knob_exactly_as_it_declares() {
    for proto in dialects() {
        let mut with = base();
        with.reasoning = Some(ReasoningEffort::High);
        let changed = enc(proto, &with) != enc(proto, &base());
        assert_eq!(
            proto.tuning().effort,
            changed,
            "a dialect's `tuning().effort` disagrees with its own encode"
        );
    }
}

/// `tuning().priority` ⇔ setting `req.service_tier` changes what this dialect encodes.
/// The two narrowing dialects (Google, Ollama) and the exec one drop it silently —
/// declared, so the listing can say so without guessing (providers.md §6.2).
#[test]
fn every_dialect_projects_the_lane_knob_exactly_as_it_declares() {
    for proto in dialects() {
        let mut with = base();
        with.service_tier = Some(ServiceTier::Priority);
        let changed = enc(proto, &with) != enc(proto, &base());
        assert_eq!(
            proto.tuning().priority,
            changed,
            "a dialect's `tuning().priority` disagrees with its own encode"
        );
    }
}

/// The declaration is DATA — cheap, pure, and identical on every call, so the read
/// surface can ask it per row without a cache (and `Tuning`'s derives are exercised).
#[test]
fn the_declaration_is_a_stable_value() {
    for proto in dialects() {
        assert_eq!(proto.tuning(), proto.tuning());
        assert!(!format!("{:?}", proto.tuning()).is_empty());
    }
    // Every shipped dialect reasons; only some have a lane. The listing's interesting
    // bit is therefore the per-row decline, which `list_providers` covers.
    assert!(dialects().iter().all(|p| p.tuning().effort));
    assert!(!OllamaChat.tuning().priority);
    assert!(OpenAiChat.tuning().priority);
}
