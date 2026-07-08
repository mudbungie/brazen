//! Request-FIELD serde (§3.1): `Role`/`ToolChoice`/`ReasoningEffort`/`OutputFormat`, the
//! string-or-sequence `content` decode, and round-trips of the full request incl. defaults
//! and the `extra` passthrough valve. The Content/media-source repr lives in the sibling
//! `canonical_request_media`; the `Tool` pair in `canonical_request_tool`.

use crate::{
    CanonicalRequest, Content, Message, OutputFormat, ReasoningEffort, Role, Tool, ToolChoice,
};
use serde_json::json;

fn rt<T>(v: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

#[test]
fn role_serializes_lowercase() {
    for (role, wire) in [
        (Role::System, "\"system\""),
        (Role::User, "\"user\""),
        (Role::Assistant, "\"assistant\""),
        (Role::Tool, "\"tool\""),
    ] {
        assert_eq!(serde_json::to_string(&role).unwrap(), wire);
        assert_eq!(rt(&role), role);
    }
}

#[test]
fn tool_choice_variants_and_default() {
    assert_eq!(ToolChoice::default(), ToolChoice::Auto);
    for (tc, wire) in [
        (ToolChoice::Auto, r#"{"type":"auto"}"#),
        (ToolChoice::Any, r#"{"type":"any"}"#),
        (ToolChoice::None, r#"{"type":"none"}"#),
        (
            ToolChoice::Tool {
                name: "search".into(),
            },
            r#"{"type":"tool","name":"search"}"#,
        ),
    ] {
        assert_eq!(serde_json::to_string(&tc).unwrap(), wire);
        assert_eq!(rt(&tc), tc);
    }
}

#[test]
fn message_content_decodes_from_string_object_or_sequence() {
    // bare string -> visit_str
    let m: Message = serde_json::from_str(r#"{"role":"user","content":"hello"}"#).unwrap();
    assert_eq!(m.content, vec![Content::Text("hello".into())]);
    // single object -> visit_map
    let m: Message =
        serde_json::from_str(r#"{"role":"user","content":{"type":"text","text":"hi"}}"#).unwrap();
    assert_eq!(m.content, vec![Content::Text("hi".into())]);
    // sequence -> visit_seq
    let m: Message =
        serde_json::from_str(r#"{"role":"assistant","content":[{"type":"text","text":"a"},"b"]}"#)
            .unwrap();
    assert_eq!(
        m.content,
        vec![Content::Text("a".into()), Content::Text("b".into())]
    );
    assert_eq!(rt(&m), m);
}

#[test]
fn tool_result_content_accepts_a_bare_string_and_defaults_is_error() {
    let c: Content = serde_json::from_str(
        r#"{"type":"tool_result","tool_use_id":"t1","content":"plain output"}"#,
    )
    .unwrap();
    assert_eq!(
        c,
        Content::ToolResult {
            tool_use_id: "t1".into(),
            content: vec![Content::Text("plain output".into())],
            is_error: false,
        }
    );
}

#[test]
fn content_field_rejects_a_non_content_value() {
    // A number is neither a string, an object, nor a sequence — the visitor's
    // `expecting` drives the error message.
    assert!(serde_json::from_str::<Message>(r#"{"role":"user","content":42}"#).is_err());
    // A single object that is not valid content fails inside `visit_map`.
    assert!(
        serde_json::from_str::<Message>(r#"{"role":"user","content":{"type":"nope"}}"#).is_err()
    );
}

#[test]
fn reasoning_effort_strings_budgets_and_parse() {
    // serde lowercase (wire + config) and the FromStr (flag/env) agree on the spelling.
    for (effort, word, budget) in [
        (ReasoningEffort::Low, "low", 1024u32),
        (ReasoningEffort::Medium, "medium", 8192),
        (ReasoningEffort::High, "high", 24576),
    ] {
        assert_eq!(effort.as_str(), word);
        assert_eq!(effort.budget(), budget); // the shared effort→budget table (providers §6)
        assert_eq!(word.parse::<ReasoningEffort>(), Ok(effort)); // FromStr
        assert_eq!(
            serde_json::to_string(&effort).unwrap(),
            format!("\"{word}\"")
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>(&format!("\"{word}\"")).unwrap(),
            effort
        );
    }
    // Low is the Anthropic budget_tokens minimum, so every rung clears the floor.
    assert!(ReasoningEffort::Low.budget() >= 1024);
    // An unrecognized spelling fails FromStr (lifted to a usage/BadValue by callers).
    assert_eq!("xhigh".parse::<ReasoningEffort>(), Err(()));
    // Copy/Eq/Debug are exercised by deriving consumers.
    assert!(!format!("{:?}", ReasoningEffort::High).is_empty());
}

#[test]
fn request_roundtrips_and_minimal_decode_defaults() {
    let req = CanonicalRequest {
        model: "claude-3-5-sonnet".into(),
        system: Some(vec![Content::Text("be terse".into())]),
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text("hi".into())],
        }],
        tools: vec![Tool::Custom {
            name: "search".into(),
            description: None,
            input_schema: json!({}),
            strict: Some(true),
        }],
        tool_choice: ToolChoice::Any,
        parallel_tool_calls: Some(false),
        max_tokens: Some(256),
        temperature: Some(0.5),
        top_p: None,
        reasoning: Some(ReasoningEffort::High),
        stop: vec!["END".into()],
        stream: Some(true),
        output: Some(OutputFormat::JsonSchema {
            name: Some("out".into()),
            schema: json!({"type": "object"}),
            strict: Some(true),
        }),
        extra: serde_json::from_value(json!({"reasoning_effort": "high"})).unwrap(),
    };
    assert_eq!(rt(&req), req);

    // A field the request omits defaults; an unmodelled key lands in `extra`.
    let min: CanonicalRequest =
        serde_json::from_str(r#"{"model":"m","safetySettings":[1]}"#).unwrap();
    assert_eq!(min.model, "m");
    assert_eq!(min.messages, Vec::new());
    assert_eq!(min.tool_choice, ToolChoice::Auto);
    assert_eq!(min.parallel_tool_calls, None); // omitted = provider default
    assert_eq!(min.stream, None); // omitted = absent, filled from config
    assert_eq!(min.output, None); // omitted = plain text (the empty-set path)

    assert_eq!(min.extra.get("safetySettings"), Some(&json!([1])));

    assert_eq!(CanonicalRequest::default(), CanonicalRequest::default());
}

#[test]
fn output_format_tagged_on_type_and_omits_absent_name_strict() {
    // `Json` is the unit variant → `{"type":"json"}`; `JsonSchema` tags on `type` and
    // drops `name`/`strict` when None (the OpenAI-only fields), so a bare schema stays lean.
    let json_mode = OutputFormat::Json;
    assert_eq!(
        serde_json::to_string(&json_mode).unwrap(),
        r#"{"type":"json"}"#
    );
    assert_eq!(rt(&json_mode), json_mode);

    let bare = OutputFormat::JsonSchema {
        name: None,
        schema: json!({"type": "object"}),
        strict: None,
    };
    assert_eq!(
        serde_json::to_string(&bare).unwrap(),
        r#"{"type":"json_schema","schema":{"type":"object"}}"#
    );
    assert_eq!(rt(&bare), bare);

    let full = OutputFormat::JsonSchema {
        name: Some("Person".into()),
        schema: json!({"type": "object"}),
        strict: Some(true),
    };
    assert_eq!(rt(&full), full);
    // As a request field: serde default None; an old request without `output` parses.
    let req: CanonicalRequest = serde_json::from_str(r#"{"model":"m"}"#).unwrap();
    assert_eq!(req.output, None);
    let carried: CanonicalRequest =
        serde_json::from_str(r#"{"model":"m","output":{"type":"json"}}"#).unwrap();
    assert_eq!(carried.output, Some(OutputFormat::Json));
}
