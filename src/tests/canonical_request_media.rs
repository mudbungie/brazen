//! Content / media-source serde (§3.1): the custom `Content` repr (bare string vs
//! tagged object), the round-trip of every content variant (incl. `Document`), and the
//! `kind`-tagged `ImageSource`/`DocumentSource` wire shape. The request-FIELD serde
//! (Role/ToolChoice/ReasoningEffort/OutputFormat/full request) lives in the sibling
//! `canonical_request`; the `Tool` pair in `canonical_request_tool`.

use crate::{Content, DocumentSource, ImageSource};
use serde_json::json;

fn rt<T>(v: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
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
        Content::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".into(),
                data: "JVBER".into(),
            },
        },
        // The `url` document source round-trips too (its `kind`-tag serde matches Image).
        Content::Document {
            source: DocumentSource::Url {
                url: "https://x/y.pdf".into(),
            },
        },
        Content::ToolUse {
            id: "t1".into(),
            name: "search".into(),
            input: json!({"q": "rust"}),
            signature: Some("gSig==".into()), // Google thoughtSignature round-trips (bl-61a9)
        },
        Content::ToolResult {
            tool_use_id: "t1".into(),
            content: vec![Content::Text("done".into())],
            is_error: false,
        },
        Content::Thinking {
            text: "reasoning".into(),
            signature: Some("sig".into()),
            id: None,
            encrypted_content: None,
        },
        Content::Thinking {
            text: "reasoning".into(),
            signature: None,
            id: None,
            encrypted_content: None,
        },
        // OpenAI Responses reasoning replay: id + encrypted_content round-trip (bl-61a9)
        Content::Thinking {
            text: "r".into(),
            signature: None,
            id: Some("rs_1".into()),
            encrypted_content: Some("ENC==".into()),
        },
        Content::RedactedThinking {
            data: "opaque".into(),
        },
        Content::ServerToolUse {
            id: "srvtoolu_1".into(),
            name: "web_search".into(),
            input: json!({"query": "weather NY"}),
        },
        Content::ServerToolResult {
            kind: "web_search_tool_result".into(),
            tool_use_id: "srvtoolu_1".into(),
            content: json!([{"type": "web_search_result", "url": "https://x"}]),
        },
        // The suffix rule generalizes: a tag brazen has never seen round-trips too.
        Content::ServerToolResult {
            kind: "code_execution_tool_result".into(),
            tool_use_id: "srvtoolu_2".into(),
            content: json!({"type": "code_execution_result", "stdout": "hi"}),
        },
    ];
    for c in variants {
        assert_eq!(rt(&c), c, "round-trip {c:?}");
    }
    // The `!= "tool_result"` guard: a client tool_result still decodes CLIENT-side.
    let client: Content = serde_json::from_str(
        r#"{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"}]}"#,
    )
    .unwrap();
    assert!(matches!(client, Content::ToolResult { .. }));
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
fn media_sources_are_tagged_on_kind() {
    // `ImageSource` and `DocumentSource` share the identical `kind`-tagged wire repr.
    let url = ImageSource::Url {
        url: "https://x/y.png".into(),
    };
    assert_eq!(
        serde_json::to_string(&url).unwrap(),
        r#"{"kind":"url","url":"https://x/y.png"}"#
    );
    assert_eq!(rt(&url), url);
    let doc = DocumentSource::Url {
        url: "https://x/y.pdf".into(),
    };
    assert_eq!(
        serde_json::to_string(&doc).unwrap(),
        r#"{"kind":"url","url":"https://x/y.pdf"}"#
    );
    assert_eq!(rt(&doc), doc);
}
