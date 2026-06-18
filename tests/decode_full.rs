//! Golden non-stream decode for the STRUCTURELESS dialects (config §4.2): `ollama`,
//! `google_genai`, and `openai` chat each fold a COMPLETE `stream:false` 2xx body to
//! the SAME canonical events their streamed form yields (explode→replay), reusing the
//! protocol's own `decode`-internal helpers rather than a second parser. The
//! explicit-structure dialects (`anthropic`, `openai_responses`) live in
//! `decode_full_structured`. No network — the body is a single aggregate JSON.

mod decode_full_support;

use brazen::protocol::google_genai::GoogleGenAi;
use brazen::protocol::ollama_chat::OllamaChat;
use brazen::protocol::openai::OpenAiChat;
use brazen::{ContentKind, Event, FinishReason, Role, Usage};
use decode_full_support::*;

#[test]
fn ollama_nonstream_folds_text_tool_usage_and_finish() {
    // The single chat object carries the WHOLE message + done:true — `line` folds it
    // as-is (the explode is the identity), so text opens, the whole tool call
    // synthesizes its id, and the open tool block promotes the finish to ToolUse.
    let body = include_bytes!("fixtures/ollama_chat_nonstream.json");
    let (ev, term) = full(&OllamaChat, body);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            Event::message_start(None, Some("llama3.2".into()), Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            tdelta(0, "Hello"),
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(1, "{\"location\":\"Paris\"}"),
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            Event::Usage(Usage {
                input: Some(12),
                output: Some(2),
                cache_read: None,
                cache_write: None,
            }),
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}

#[test]
fn google_nonstream_folds_text_tool_and_finish() {
    // The single GenerateContentResponse carries all parts + finishReason — `chunk`
    // folds it as the terminal chunk (the explode is the identity).
    let body = include_bytes!("fixtures/google_genai_nonstream.json");
    let (ev, term) = full(&GoogleGenAi, body);
    assert!(term);
    assert_eq!(
        ev,
        vec![
            Event::message_start(None, Some("gemini-2.0".into()), Role::Assistant),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            tdelta(0, "Hello"),
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(1, "{\"location\":\"Paris\"}"),
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            Event::Usage(Usage {
                input: Some(12),
                output: Some(8),
                cache_read: Some(3),
                cache_write: None,
            }),
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::End,
        ]
    );
}

#[test]
fn openai_chat_nonstream_folds_message_two_tools_and_finish() {
    // The non-stream `choices[0].message` projects onto one synthetic delta: the
    // whole content is one TextDelta, and each `tool_calls[]` gets its ARRAY POSITION
    // as the wire index so the two calls keep DISTINCT blocks (the absent index would
    // collide them). Finish lands before the same-frame Usage (the streamed order).
    let body = include_bytes!("fixtures/openai_chat_nonstream.json");
    let (ev, term) = full(&OpenAiChat, body);
    assert!(!term); // openai chat's terminator is the separate `[DONE]`, never in-body
    assert_eq!(
        ev,
        vec![
            Event::message_start(
                Some("chatcmpl-9".into()),
                Some("gpt-4o-2024-08-06".into()),
                Role::Assistant,
            ),
            Event::ContentStart {
                index: 0,
                kind: ContentKind::Text {}
            },
            tdelta(0, "Hello"),
            Event::ContentStart {
                index: 1,
                kind: ContentKind::ToolUse {
                    id: "call_a".into(),
                    name: "get_weather".into(),
                },
            },
            jdelta(1, "{\"city\":\"SF\"}"),
            Event::ContentStart {
                index: 2,
                kind: ContentKind::ToolUse {
                    id: "call_b".into(),
                    name: "get_time".into(),
                },
            },
            jdelta(2, "{\"tz\":\"PT\"}"),
            Event::ContentStop { index: 0 },
            Event::ContentStop { index: 1 },
            Event::ContentStop { index: 2 },
            Event::Finish {
                reason: FinishReason::ToolUse
            },
            Event::Usage(Usage {
                input: Some(12),
                output: Some(8),
                cache_read: Some(4),
                cache_write: None,
            }),
            Event::End,
        ]
    );
}
