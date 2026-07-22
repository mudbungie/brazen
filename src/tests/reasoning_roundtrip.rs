//! End-to-end reasoning round-trip (bl-61a9): a golden wire fixture → canonical
//! `Event`s → the harness-side fold → a canonical assistant transcript → `encode`
//! → wire bytes carrying the provider's opaque replay payload VERBATIM. One test
//! per dialect proves the marquee `--json` agent-loop path — decode CAPTURES the
//! blob, encode REPLAYS it — for Anthropic (thinking signature + redacted data),
//! Google (functionCall thoughtSignature), and OpenAI Responses (reasoning item
//! id + encrypted_content). No network.

use std::collections::BTreeMap;

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::tests::decode_full_support::full;
use crate::{
    CanonicalRequest, Content, ContentKind, DecodeState, Delta, Event, Framing, Message, Protocol,
    ProviderCtx, Role,
};
use serde_json::{json, Value};

/// The harness-side fold a `--json` consumer runs: replay `Event`s into the
/// `Vec<Content>` of the assistant turn it will re-send next round. Index-keyed, so
/// a `SignatureDelta` lands on its own block regardless of interleaving.
fn fold(events: &[Event]) -> Vec<Content> {
    enum B {
        Text(String),
        Tool {
            id: String,
            name: String,
            args: String,
            sig: Option<String>,
        },
        Think {
            text: String,
            sig: Option<String>,
            id: Option<String>,
            enc: Option<String>,
        },
        Redacted(String),
        Skip,
    }
    let mut blocks: BTreeMap<u32, B> = BTreeMap::new();
    let mut out = Vec::new();
    for ev in events {
        match ev {
            Event::ContentStart { index, kind } => {
                blocks.insert(
                    *index,
                    match kind {
                        ContentKind::Text {} => B::Text(String::new()),
                        ContentKind::ToolUse { id, name } => B::Tool {
                            id: id.clone(),
                            name: name.clone(),
                            args: String::new(),
                            sig: None,
                        },
                        ContentKind::Thinking { id } => B::Think {
                            text: String::new(),
                            sig: None,
                            id: id.clone(),
                            enc: None,
                        },
                        ContentKind::RedactedThinking { data } => B::Redacted(data.clone()),
                        _ => B::Skip,
                    },
                );
            }
            Event::ContentDelta { index, delta } => {
                let Some(b) = blocks.get_mut(index) else {
                    continue;
                };
                match (b, delta) {
                    (B::Text(s), Delta::TextDelta(t)) => s.push_str(t),
                    (B::Think { text, .. }, Delta::ThinkingDelta(t)) => text.push_str(t),
                    (B::Tool { args, .. }, Delta::JsonDelta(t)) => args.push_str(t),
                    (B::Think { sig, .. } | B::Tool { sig, .. }, Delta::SignatureDelta(s)) => {
                        *sig = Some(s.clone())
                    }
                    (B::Think { enc, .. }, Delta::EncryptedReasoningDelta(e)) => {
                        *enc = Some(e.clone())
                    }
                    _ => {}
                }
            }
            Event::ContentStop { index } => {
                if let Some(b) = blocks.remove(index) {
                    match b {
                        B::Text(s) => out.push(Content::Text(s)),
                        B::Tool {
                            id,
                            name,
                            args,
                            sig,
                        } => out.push(Content::ToolUse {
                            id,
                            name,
                            input: serde_json::from_str(&args).unwrap_or(Value::Null),
                            signature: sig,
                        }),
                        B::Think { text, sig, id, enc } => out.push(Content::Thinking {
                            text,
                            signature: sig,
                            id,
                            encrypted_content: enc,
                        }),
                        B::Redacted(data) => out.push(Content::RedactedThinking { data }),
                        B::Skip => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Decode a streamed SSE fixture to its `Event`s (the streaming counterpart of
/// `decode_full_support::full`, used for the Google fixture).
fn stream(proto: &dyn Protocol, bytes: &[u8]) -> Vec<Event> {
    let mut dec = Framing::Sse.decoder();
    let mut frames = dec.push(bytes.to_vec()).unwrap();
    frames.extend(dec.finish().unwrap());
    let mut state = DecodeState::default();
    frames
        .into_iter()
        .flat_map(|f| proto.decode(f, &mut state).unwrap())
        .collect()
}

/// Encode a single assistant turn of `content` and return its parsed wire body.
fn encode_assistant(proto: &dyn Protocol, ctx: &ProviderCtx, content: Vec<Content>) -> Value {
    let req = CanonicalRequest {
        max_tokens: Some(256),
        messages: vec![Message {
            role: Role::Assistant,
            content,
        }],
        ..Default::default()
    };
    serde_json::from_slice(&proto.encode(&req, ctx).unwrap().body).unwrap()
}

fn ctx<'a>(base_url: &'a str, model: &'a str) -> ProviderCtx<'a> {
    ProviderCtx {
        base_url,
        model,
        beta_headers: &[],
        exec: None,
    }
}

#[test]
fn anthropic_thinking_signature_and_redacted_data_round_trip() {
    let (events, _) = full(
        &AnthropicMessages,
        include_bytes!("../../tests/fixtures/anthropic_messages_nonstream.json"),
    );
    let content = fold(&events);
    let body = encode_assistant(&AnthropicMessages, &ctx("https://api", "claude"), content);
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    // Both opaque blobs ride the re-encoded assistant turn VERBATIM.
    assert!(blocks.contains(&json!({
        "type": "thinking", "thinking": "Let me check.", "signature": "EqQB=="
    })));
    assert!(blocks.contains(&json!({ "type": "redacted_thinking", "data": "AAABBB==" })));
}

#[test]
fn google_thought_signature_round_trips_on_the_function_call_part() {
    let events = stream(
        &GoogleGenAi,
        include_bytes!("../../tests/fixtures/google_genai_tools.sse"),
    );
    let content = fold(&events);
    // The fold recovered the thoughtSignature onto ToolUse.signature.
    assert_eq!(
        content,
        vec![Content::ToolUse {
            id: "call_0".into(),
            name: "get_weather".into(),
            input: json!({ "location": "Paris" }),
            signature: Some("gSig==".into()),
        }]
    );
    let body = encode_assistant(
        &GoogleGenAi,
        &ctx("https://gen", "gemini-1.5-flash"),
        content,
    );
    // encode re-emits it as the functionCall part's thoughtSignature sibling — the
    // LOAD-BEARING field Gemini 2.5 multi-turn function calling 400s without.
    let parts = body["contents"][0]["parts"].as_array().unwrap();
    assert!(parts.contains(&json!({
        "functionCall": { "name": "get_weather", "args": { "location": "Paris" } },
        "thoughtSignature": "gSig=="
    })));
}

#[test]
fn responses_reasoning_item_id_and_encrypted_content_round_trip() {
    let (events, _) = full(
        &OpenAiResponses,
        include_bytes!("../../tests/fixtures/openai_responses_nonstream.json"),
    );
    let content = fold(&events);
    // The reasoning item folded to Thinking with id + encrypted_content captured.
    assert!(content.contains(&Content::Thinking {
        text: "Thinking.".into(),
        signature: None,
        id: Some("rs_1".into()),
        encrypted_content: Some("ENC==".into()),
    }));
    let body = encode_assistant(&OpenAiResponses, &ctx("https://api", "gpt"), content);
    let input = body["input"].as_array().unwrap();
    // encode reconstructs a reasoning input item for stateless (store:false) replay.
    assert!(input.contains(&json!({
        "type": "reasoning", "id": "rs_1",
        "summary": [{ "type": "summary_text", "text": "Thinking." }],
        "encrypted_content": "ENC=="
    })));
}
