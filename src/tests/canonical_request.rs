//! Request model tests (§3.1): the custom `Content` repr (bare string vs tagged
//! object), the string-or-sequence `content` decode, and round-trips of every
//! request type incl. defaults and the `extra` passthrough valve.

use crate::{
    CacheAnchor, CacheBreakpoint, CacheTtl, CanonicalRequest, Content, ImageSource, Message,
    ReasoningEffort, Role, Tool, ToolChoice,
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
fn content_every_variant_roundtrips() {
    let variants = [
        Content::Text("hi".into()),
        Content::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "AAAA".into(),
            },
        },
        Content::ToolUse {
            id: "t1".into(),
            name: "search".into(),
            input: json!({"q": "rust"}),
        },
        Content::ToolResult {
            tool_use_id: "t1".into(),
            content: vec![Content::Text("done".into())],
            is_error: false,
        },
        Content::Thinking {
            text: "reasoning".into(),
            signature: Some("sig".into()),
        },
        Content::Thinking {
            text: "reasoning".into(),
            signature: None,
        },
        Content::RedactedThinking {
            data: "opaque".into(),
        },
    ];
    for c in variants {
        assert_eq!(rt(&c), c, "round-trip {c:?}");
    }
}

#[test]
fn text_serializes_as_a_tagged_object_but_decodes_from_a_bare_string() {
    let c = Content::Text("hi".into());
    assert_eq!(
        serde_json::to_string(&c).unwrap(),
        r#"{"type":"text","text":"hi"}"#
    );
    let bare: Content = serde_json::from_str(r#""hi""#).unwrap();
    assert_eq!(bare, Content::Text("hi".into()));
}

#[test]
fn image_source_is_tagged_on_kind() {
    let url = ImageSource::Url {
        url: "https://x/y.png".into(),
    };
    assert_eq!(
        serde_json::to_string(&url).unwrap(),
        r#"{"kind":"url","url":"https://x/y.png"}"#
    );
    assert_eq!(rt(&url), url);
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
fn tool_roundtrips_with_and_without_description() {
    let with = Tool {
        name: "search".into(),
        description: Some("web search".into()),
        input_schema: json!({"type": "object"}),
    };
    assert_eq!(rt(&with), with);
    // description defaults to None when the key is absent.
    let bare: Tool =
        serde_json::from_str(r#"{"name":"x","input_schema":{"type":"object"}}"#).unwrap();
    assert_eq!(bare.description, None);
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
        tools: vec![Tool {
            name: "search".into(),
            description: None,
            input_schema: json!({}),
        }],
        tool_choice: ToolChoice::Any,
        parallel_tool_calls: Some(false),
        max_tokens: Some(256),
        temperature: Some(0.5),
        top_p: None,
        reasoning: Some(ReasoningEffort::High),
        stop: vec!["END".into()],
        stream: Some(true),
        cache: vec![CacheBreakpoint {
            anchor: CacheAnchor::Message { index: 0 },
            ttl: CacheTtl::OneHour,
        }],
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
    assert_eq!(min.cache, Vec::new()); // omitted = no caching (the general empty path)

    assert_eq!(min.extra.get("safetySettings"), Some(&json!([1])));

    assert_eq!(CanonicalRequest::default(), CanonicalRequest::default());
}

#[test]
fn cache_types_serde_spellings_and_defaults() {
    // CacheAnchor: flattened snake_case `anchor` tag; `Message` carries `index`.
    for (anchor, wire) in [
        (CacheAnchor::Tools, r#"{"anchor":"tools"}"#),
        (CacheAnchor::System, r#"{"anchor":"system"}"#),
        (
            CacheAnchor::Message { index: 2 },
            r#"{"anchor":"message","index":2}"#,
        ),
    ] {
        assert_eq!(serde_json::to_string(&anchor).unwrap(), wire);
        assert_eq!(rt(&anchor), anchor);
    }
    // CacheTtl: the two renames `"5m"`/`"1h"` are the one home for the spellings.
    assert_eq!(serde_json::to_string(&CacheTtl::FiveMin).unwrap(), "\"5m\"");
    assert_eq!(serde_json::to_string(&CacheTtl::OneHour).unwrap(), "\"1h\"");
    assert_eq!(
        serde_json::from_str::<CacheTtl>("\"5m\"").unwrap(),
        CacheTtl::FiveMin
    );
    assert_eq!(
        serde_json::from_str::<CacheTtl>("\"1h\"").unwrap(),
        CacheTtl::OneHour
    );
    assert_eq!(CacheTtl::default(), CacheTtl::FiveMin);
    // A breakpoint is a single flat object; an omitted `ttl` defaults to FiveMin.
    let bp = CacheBreakpoint {
        anchor: CacheAnchor::Tools,
        ttl: CacheTtl::FiveMin,
    };
    assert_eq!(
        serde_json::to_string(&bp).unwrap(),
        r#"{"anchor":"tools","ttl":"5m"}"#
    );
    let decoded: CacheBreakpoint = serde_json::from_str(r#"{"anchor":"tools"}"#).unwrap();
    assert_eq!(decoded, bp);
    assert_eq!(rt(&bp), bp);
}
