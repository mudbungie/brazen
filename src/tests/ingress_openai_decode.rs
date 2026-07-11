//! `openai_chat` ingress decode goldens (ingress.md §14): dialect-request fixtures →
//! the exact `CanonicalRequest`, plus the per-field happy paths of the inverse
//! mapping (openai-chat-mapping §2, read right-to-left). Error rungs live in
//! `ingress_openai_errors`; the decode∘encode property in `ingress_roundtrip`.

use serde_json::{json, Value};

use crate::{
    decode_request, CanonicalRequest, Content, DocumentSource, ImageSource, IngressId, Message,
    OutputFormat, ReasoningEffort, Role, Tool, ToolChoice,
};

const FULL: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_request_full.json");
const MINIMAL: &[u8] = include_bytes!("../../tests/fixtures/openai_chat_request_minimal.json");

fn dec(v: Value) -> CanonicalRequest {
    decode_request(IngressId::OpenAiChat, v.to_string().as_bytes()).unwrap()
}

fn text(s: &str) -> Content {
    Content::Text(s.into())
}

#[test]
fn minimal_fixture_decodes_to_defaults() {
    let req = decode_request(IngressId::OpenAiChat, MINIMAL).unwrap();
    let mut want = CanonicalRequest {
        model: "bz-smoke".into(),
        ..Default::default()
    };
    want.messages.push(Message {
        role: Role::User,
        content: vec![text("hi")],
    });
    assert_eq!(req, want); // stream/system/output all None; tool_choice Auto; extra empty
}

#[test]
fn full_fixture_decodes_every_field() {
    let req = decode_request(IngressId::OpenAiChat, FULL).unwrap();
    assert_eq!(req.model, "gpt-4o");
    assert_eq!(req.system, Some(vec![text("You are concise.")])); // leading system message
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
                            data: "iVBORw0KG==".into()
                        }
                    },
                    Content::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/a.png".into()
                        }
                    },
                    Content::Document {
                        source: DocumentSource::Base64 {
                            media_type: "application/pdf".into(),
                            data: "JVBERi0=".into()
                        }
                    },
                ],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    text("Checking."),
                    Content::ToolUse {
                        id: "call_abc".into(),
                        name: "get_weather".into(),
                        input: json!({"location": "Paris"}),
                        signature: None,
                    },
                ],
            },
            // two consecutive wire tool messages coalesce into ONE Role::Tool turn;
            // the "[error] " prefix lifts back to the structural is_error flag.
            Message {
                role: Role::Tool,
                content: vec![
                    Content::ToolResult {
                        tool_use_id: "call_abc".into(),
                        content: vec![text("18C, clear")],
                        is_error: false,
                    },
                    Content::ToolResult {
                        tool_use_id: "call_def".into(),
                        content: vec![text("station offline")],
                        is_error: true,
                    },
                ],
            },
            // a NON-leading developer message stays an in-band Role::System message
            Message {
                role: Role::System,
                content: vec![text("Prefer metric.")],
            },
        ]
    );
    assert_eq!(
        req.tools,
        vec![Tool::Custom {
            name: "get_weather".into(),
            description: Some("Current weather".into()),
            input_schema: json!({"type": "object",
                "properties": {"location": {"type": "string"}}, "required": ["location"]}),
            strict: Some(true),
        }]
    );
    assert_eq!(
        req.tool_choice,
        ToolChoice::Tool {
            name: "get_weather".into()
        }
    );
    assert_eq!(req.parallel_tool_calls, Some(false));
    assert_eq!(req.max_tokens, Some(512));
    assert_eq!(req.temperature, Some(0.5));
    assert_eq!(req.top_p, Some(0.9));
    assert_eq!(req.reasoning, Some(ReasoningEffort::High));
    assert_eq!(req.stop, vec!["STOP".to_string()]);
    assert_eq!(req.stream, Some(true));
    assert_eq!(
        req.output,
        Some(OutputFormat::JsonSchema {
            name: Some("weather".into()),
            schema: json!({"type": "object"}),
            strict: Some(true),
        })
    );
    // unknown top-level keys ride `extra` verbatim — including the client's
    // stream_options, kept for the response encoder's shape decision.
    assert_eq!(req.extra.get("seed"), Some(&json!(42)));
    assert_eq!(
        req.extra.get("stream_options"),
        Some(&json!({"include_usage": true}))
    );
    assert_eq!(req.extra.len(), 2);
}

#[test]
fn tool_choice_string_spellings_and_absent_default() {
    let tc = |v: Value| dec(json!({"messages": [], "tool_choice": v})).tool_choice;
    assert_eq!(tc(json!("auto")), ToolChoice::Auto);
    assert_eq!(tc(json!("required")), ToolChoice::Any);
    assert_eq!(tc(json!("none")), ToolChoice::None);
    assert_eq!(dec(json!({"messages": []})).tool_choice, ToolChoice::Auto);
}

#[test]
fn scalar_shapes() {
    // a bare-string stop is the array of one; max_completion_tokens is the SAME
    // canonical fact as max_tokens (the encoder re-picks the key, §2.7).
    let req = dec(json!({"stop": "END", "max_completion_tokens": 64, "stream": false}));
    assert_eq!(req.stop, vec!["END".to_string()]);
    assert_eq!(req.max_tokens, Some(64));
    assert_eq!(req.stream, Some(false));
    assert_eq!(req.model, ""); // absent model = empty (config fills it later)
    let req = dec(json!({"reasoning_effort": "low"}));
    assert_eq!(req.reasoning, Some(ReasoningEffort::Low));
}

#[test]
fn response_format_text_and_json_object_and_bare_schema() {
    assert_eq!(
        dec(json!({"response_format": {"type": "text"}})).output,
        None
    );
    assert_eq!(
        dec(json!({"response_format": {"type": "json_object"}})).output,
        Some(OutputFormat::Json)
    );
    // name/schema/strict all absent: None name, Null schema, None strict — never fabricated
    assert_eq!(
        dec(json!({"response_format": {"type": "json_schema", "json_schema": {"strict": null}}}))
            .output,
        Some(OutputFormat::JsonSchema {
            name: None,
            schema: Value::Null,
            strict: None,
        })
    );
}

#[test]
fn assistant_content_shapes() {
    let content = |m: Value| dec(json!({"messages": [m]})).messages.remove(0).content;
    // the encoder's "" fabrication inverts to NO parts; null and absent likewise
    assert_eq!(content(json!({"role": "assistant", "content": ""})), vec![]);
    assert_eq!(
        content(json!({"role": "assistant", "content": null})),
        vec![]
    );
    assert_eq!(content(json!({"role": "assistant"})), vec![]);
    assert_eq!(
        content(json!({"role": "assistant", "content": [{"type": "text", "text": "hi"}]})),
        vec![text("hi")]
    );
}

#[test]
fn tool_call_argument_shapes() {
    let call = |args: Value| {
        let m = json!({"role": "assistant", "tool_calls": [
            {"id": "c1", "function": {"name": "f", "arguments": args}}]});
        dec(json!({"messages": [m]}))
            .messages
            .remove(0)
            .content
            .remove(0)
    };
    let input = |c: Content| match c {
        Content::ToolUse { input, .. } => input,
        other => panic!("not a tool use: {other:?}"),
    };
    assert_eq!(input(call(json!(""))), json!({})); // ""-arguments convention → {}
    assert_eq!(input(call(json!("{\"a\":1}"))), json!({"a": 1}));
    assert_eq!(input(call(json!({"a": 1}))), json!({"a": 1})); // object accepted verbatim

    // an absent arguments key is the same empty-input fact as ""
    let m = json!({"role": "assistant", "tool_calls": [{"id": "c1", "function": {"name": "f"}}]});
    let c = dec(json!({"messages": [m]}))
        .messages
        .remove(0)
        .content
        .remove(0);
    assert_eq!(input(c), json!({}));
}

#[test]
fn tool_messages_do_not_coalesce_across_other_roles() {
    let req = dec(json!({"messages": [
        {"role": "tool", "tool_call_id": "c1", "content": "r1"},
        {"role": "user", "content": "next"},
        {"role": "tool", "tool_call_id": "c2", "content": [{"type": "text", "text": "r2"}]}
    ]}));
    assert_eq!(req.messages.len(), 3); // tool, user, tool — no merge over the gap
    assert_eq!(req.messages[0].role, Role::Tool);
    assert_eq!(req.messages[2].role, Role::Tool);
    assert_eq!(
        req.messages[2].content,
        vec![Content::ToolResult {
            tool_use_id: "c2".into(),
            content: vec![text("r2")], // array-of-text tool content maps per part
            is_error: false,
        }]
    );
}

#[test]
fn tolerated_shapes() {
    // a tool with no `type` key is the unambiguous default; parameters/description/
    // strict absent → Null schema / None / None, never fabricated
    let req = dec(json!({"tools": [{"function": {"name": "f"}}]}));
    assert_eq!(
        req.tools,
        vec![Tool::Custom {
            name: "f".into(),
            description: None,
            input_schema: Value::Null,
            strict: None,
        }]
    );
    // a non-base64 data URI is NOT the encoder's embedding — it stays a Url verbatim
    let req = dec(json!({"messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:text/plain,hello"}}]}]}));
    assert_eq!(
        req.messages[0].content,
        vec![Content::Image {
            source: ImageSource::Url {
                url: "data:text/plain,hello".into()
            }
        }]
    );
    // a system message with array-of-text content; a LEADING one is req.system
    let req = dec(json!({"messages": [
        {"role": "developer", "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}
    ]}));
    assert_eq!(req.system, Some(vec![text("a"), text("b")]));
    assert!(req.messages.is_empty());
}
