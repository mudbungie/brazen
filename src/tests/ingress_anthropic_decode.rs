//! `anthropic_messages` ingress decode goldens (ingress.md §14): dialect-request
//! fixtures → the exact `CanonicalRequest`, plus the per-field happy paths of the
//! inverse mapping (anthropic-messages §2, read right-to-left). Error rungs live in
//! `ingress_anthropic_errors`; the decode∘encode property in `ingress_anthropic_roundtrip`.

use serde_json::{json, Value};

use crate::{
    decode_request, CanonicalRequest, Content, DocumentSource, ImageSource, IngressId, Message,
    OutputFormat, Role, Tool, ToolChoice,
};

const FULL: &[u8] = include_bytes!("../../tests/fixtures/anthropic_messages_request_full.json");
const MINIMAL: &[u8] =
    include_bytes!("../../tests/fixtures/anthropic_messages_request_minimal.json");

fn dec(v: Value) -> CanonicalRequest {
    decode_request(IngressId::AnthropicMessages, v.to_string().as_bytes()).unwrap()
}

fn text(s: &str) -> Content {
    Content::Text(s.into())
}

#[test]
fn minimal_fixture_decodes_to_defaults() {
    let req = decode_request(IngressId::AnthropicMessages, MINIMAL).unwrap();
    let want = CanonicalRequest {
        model: "claude-sonnet-4-6".into(),
        max_tokens: Some(1024),
        messages: vec![Message {
            role: Role::User,
            content: vec![text("hi")], // a bare string content → one Text
        }],
        ..Default::default()
    };
    assert_eq!(req, want); // stream/system/output None; tool_choice Auto; extra empty
}

#[test]
fn full_fixture_decodes_every_field() {
    let req = decode_request(IngressId::AnthropicMessages, FULL).unwrap();
    assert_eq!(req.model, "claude-opus-4-8");
    assert_eq!(req.max_tokens, Some(1024));
    assert_eq!(req.system, Some(vec![text("You are a terse weather bot.")]));
    assert_eq!(req.temperature, Some(0.7));
    assert_eq!(req.top_p, Some(0.9));
    assert_eq!(req.stop, vec!["\n\nHuman:".to_owned()]); // stop_sequences → stop
    assert_eq!(req.stream, Some(true));
    assert_eq!(req.tool_choice, ToolChoice::Auto);
    assert_eq!(req.parallel_tool_calls, Some(false)); // disable_parallel_tool_use lifted out
    assert_eq!(
        req.output,
        Some(OutputFormat::JsonSchema {
            name: None, // schema-only wire → name/strict narrowed to None
            schema: json!({"type": "object"}),
            strict: None,
        })
    );
    // the wire `thinking` knob and `metadata` ride extra verbatim (no typed home)
    assert_eq!(
        req.extra["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(req.extra["metadata"], json!({"user_id": "u1"}));
    assert!(req.reasoning.is_none()); // thinking does NOT reconstruct reasoning (§5 idle)
}

#[test]
fn full_fixture_decodes_the_transcript_and_tools() {
    let req = decode_request(IngressId::AnthropicMessages, FULL).unwrap();
    assert_eq!(
        req.messages,
        vec![
            Message {
                role: Role::User,
                content: vec![
                    text("What's in this image, and the weather in Paris?"),
                    Content::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: "iVBORw0KG==".into(),
                        },
                    },
                    Content::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/a.png".into(),
                        },
                    },
                    Content::Document {
                        source: DocumentSource::Base64 {
                            media_type: "application/pdf".into(),
                            data: "JVBERi0=".into(),
                        },
                    },
                ],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    Content::Thinking {
                        text: "Think.".into(),
                        signature: Some("SIG==".into()),
                        id: None,
                        encrypted_content: None,
                    },
                    Content::ToolUse {
                        id: "toolu_01A".into(),
                        name: "get_weather".into(),
                        input: json!({"location": "Paris"}),
                        signature: None,
                    },
                ],
            },
            // the tool-result-bearing user turn re-coalesces to Role::Tool; cache_control ignored
            Message {
                role: Role::Tool,
                content: vec![Content::ToolResult {
                    tool_use_id: "toolu_01A".into(),
                    content: vec![text("18C")],
                    is_error: false,
                }],
            },
        ]
    );
    assert_eq!(
        req.tools,
        vec![
            Tool::Custom {
                name: "get_weather".into(),
                description: Some("Current weather".into()),
                input_schema: json!({"type": "object",
                    "properties": {"location": {"type": "string"}}, "required": ["location"]}),
                strict: Some(true),
            },
            Tool::Provider {
                kind: "web_search_20250305".into(),
                name: "web_search".into(),
                config: json!({"max_uses": 3}).as_object().unwrap().clone(),
            },
        ]
    );
}

#[test]
fn tool_choice_variants_and_string_stop() {
    for (wire, want) in [
        (json!({"type": "auto"}), ToolChoice::Auto),
        (json!({"type": "any"}), ToolChoice::Any),
        (json!({"type": "none"}), ToolChoice::None),
        (
            json!({"type": "tool", "name": "f"}),
            ToolChoice::Tool { name: "f".into() },
        ),
    ] {
        let req = dec(json!({"model": "m", "messages": [], "tool_choice": wire}));
        assert_eq!(req.tool_choice, want);
        assert_eq!(req.parallel_tool_calls, None); // no disable_parallel_tool_use → default
    }
    // a bare string stop_sequence decodes to the one-element canonical vec
    assert_eq!(
        dec(json!({"model": "m", "messages": [], "stop_sequences": "STOP"})).stop,
        vec!["STOP".to_owned()]
    );
}

#[test]
fn server_tool_blocks_decode_verbatim() {
    // The dialect carries server-tool blocks natively (§2.5): both round-trip verbatim,
    // the `*_tool_result` tag suffix-matched and its content kept whole.
    let req = dec(
        json!({"model": "m", "messages": [{"role": "assistant", "content": [
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"q": "x"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
             "content": [{"type": "web_search_result", "title": "T"}]}
        ]}]}),
    );
    assert_eq!(
        req.messages[0].content,
        vec![
            Content::ServerToolUse {
                id: "srvtoolu_1".into(),
                name: "web_search".into(),
                input: json!({"q": "x"}),
            },
            Content::ServerToolResult {
                kind: "web_search_tool_result".into(),
                tool_use_id: "srvtoolu_1".into(),
                content: json!([{"type": "web_search_result", "title": "T"}]),
            },
        ]
    );
}

#[test]
fn redacted_thinking_and_bare_string_tool_result_decode() {
    let req = dec(json!({"model": "m", "messages": [
        {"role": "assistant", "content": [{"type": "redacted_thinking", "data": "blob"}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "plain text result"}
        ]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t2",
             "content": [{"type": "image", "source": {"type": "url", "url": "http://x/i.png"}}]}
        ]}
    ]}));
    assert_eq!(
        req.messages[0].content,
        vec![Content::RedactedThinking {
            data: "blob".into()
        }]
    );
    // a bare-string tool_result content → one Text; is_error absent → false
    assert_eq!(
        req.messages[1].content,
        vec![Content::ToolResult {
            tool_use_id: "t1".into(),
            content: vec![text("plain text result")],
            is_error: false,
        }]
    );
    // an image block in the tool_result slot is representable
    assert_eq!(
        req.messages[2].content,
        vec![Content::ToolResult {
            tool_use_id: "t2".into(),
            content: vec![Content::Image {
                source: ImageSource::Url {
                    url: "http://x/i.png".into()
                },
            }],
            is_error: false,
        }]
    );
}

#[test]
fn a_system_string_and_missing_input_default() {
    // a bare system string → one Text; a tool_use with no `input` key → null input
    let req = dec(json!({"model": "m", "system": "Be brief.",
        "messages": [{"role": "assistant", "content": [
            {"type": "tool_use", "id": "t", "name": "f"}]}]}));
    assert_eq!(req.system, Some(vec![text("Be brief.")]));
    assert_eq!(
        req.messages[0].content,
        vec![Content::ToolUse {
            id: "t".into(),
            name: "f".into(),
            input: Value::Null,
            signature: None,
        }]
    );
}

#[test]
fn a_custom_tool_without_input_schema_defaults_null() {
    let req = dec(json!({"model": "m", "messages": [], "tools": [{"name": "bare"}]}));
    assert_eq!(
        req.tools,
        vec![Tool::Custom {
            name: "bare".into(),
            description: None,
            input_schema: Value::Null,
            strict: None,
        }]
    );
}
