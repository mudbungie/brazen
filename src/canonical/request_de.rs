//! The wire repr of [`Content`](super::Content) (CR-4): a custom serde pair so a
//! bare string (`"hi"`) and a `{"type":…}` object both decode to it, plus
//! `de_content_seq` for a `content` slot that accepts a string, one object, or a
//! sequence. Server-tool RESULT blocks carry a DYNAMIC tag: any `type` ending in
//! `_tool_result` (except the client `tool_result` itself) decodes to
//! `ServerToolResult{kind: <tag>, …}` and re-emits `kind` verbatim — the open-set
//! rule applied to result blocks. Kept apart from the type definitions — the model
//! is one concern, its lossless projection to/from the wire another. The `Tool`
//! wire pair (keyed on the presence of `type`) lives in the sibling
//! `request_de_tool`.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::{Content, ImageSource};

impl Serialize for Content {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // A borrowed, internally-tagged mirror — serializes without cloning and
        // emits the same `{"type":…}` shape the deserializer accepts.
        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Tagged<'a> {
            Text {
                text: &'a str,
            },
            Image {
                source: &'a ImageSource,
            },
            ToolUse {
                id: &'a str,
                name: &'a str,
                input: &'a serde_json::Value,
                #[serde(skip_serializing_if = "Option::is_none")]
                signature: Option<&'a str>,
            },
            ToolResult {
                tool_use_id: &'a str,
                content: &'a [Content],
                is_error: bool,
            },
            Thinking {
                text: &'a str,
                signature: &'a Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                id: Option<&'a str>,
                #[serde(skip_serializing_if = "Option::is_none")]
                encrypted_content: Option<&'a str>,
            },
            RedactedThinking {
                data: &'a str,
            },
            ServerToolUse {
                id: &'a str,
                name: &'a str,
                input: &'a serde_json::Value,
            },
        }
        // `ServerToolResult`'s tag is DYNAMIC (`kind` IS the wire `type`), so it
        // cannot be a fixed `Tagged` variant — a plain struct re-emits it verbatim.
        #[derive(Serialize)]
        struct SrvResult<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            tool_use_id: &'a str,
            content: &'a Value,
        }
        let tagged = match self {
            Content::Text(text) => Tagged::Text { text },
            Content::Image { source } => Tagged::Image { source },
            Content::ToolUse {
                id,
                name,
                input,
                signature,
            } => Tagged::ToolUse {
                id,
                name,
                input,
                signature: signature.as_deref(),
            },
            Content::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Tagged::ToolResult {
                tool_use_id,
                content,
                is_error: *is_error,
            },
            Content::Thinking {
                text,
                signature,
                id,
                encrypted_content,
            } => Tagged::Thinking {
                text,
                signature,
                id: id.as_deref(),
                encrypted_content: encrypted_content.as_deref(),
            },
            Content::RedactedThinking { data } => Tagged::RedactedThinking { data },
            Content::ServerToolUse { id, name, input } => Tagged::ServerToolUse { id, name, input },
            Content::ServerToolResult {
                kind,
                tool_use_id,
                content,
            } => {
                return SrvResult {
                    kind,
                    tool_use_id,
                    content,
                }
                .serialize(s)
            }
        };
        tagged.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Tagged {
            Text {
                text: String,
            },
            Image {
                source: ImageSource,
            },
            ToolUse {
                id: String,
                name: String,
                input: serde_json::Value,
                #[serde(default)]
                signature: Option<String>,
            },
            ToolResult {
                tool_use_id: String,
                #[serde(deserialize_with = "de_content_seq")]
                content: Vec<Content>,
                #[serde(default)]
                is_error: bool,
            },
            Thinking {
                text: String,
                #[serde(default)]
                signature: Option<String>,
                #[serde(default)]
                id: Option<String>,
                #[serde(default)]
                encrypted_content: Option<String>,
            },
            RedactedThinking {
                data: String,
            },
            ServerToolUse {
                id: String,
                name: String,
                input: serde_json::Value,
            },
        }
        // A bare string is `Text`. An object whose `type` carries the open-set
        // `*_tool_result` suffix (but is not the client `tool_result`) is a
        // server-tool RESULT — intercepted BEFORE the fixed-tag dispatch, its tag
        // carried as data. Everything else decodes by its fixed `type` tag.
        let v = Value::deserialize(d)?;
        if let Value::String(text) = v {
            return Ok(Content::Text(text));
        }
        let tag = v.get("type").and_then(Value::as_str).unwrap_or_default();
        if tag.ends_with("_tool_result") && tag != "tool_result" {
            return Ok(Content::ServerToolResult {
                kind: tag.to_owned(),
                tool_use_id: v["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                content: v["content"].clone(),
            });
        }
        Ok(match Tagged::deserialize(v).map_err(de::Error::custom)? {
            Tagged::Text { text } => Content::Text(text),
            Tagged::Image { source } => Content::Image { source },
            Tagged::ToolUse {
                id,
                name,
                input,
                signature,
            } => Content::ToolUse {
                id,
                name,
                input,
                signature,
            },
            Tagged::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Content::ToolResult {
                tool_use_id,
                content,
                is_error,
            },
            Tagged::Thinking {
                text,
                signature,
                id,
                encrypted_content,
            } => Content::Thinking {
                text,
                signature,
                id,
                encrypted_content,
            },
            Tagged::RedactedThinking { data } => Content::RedactedThinking { data },
            Tagged::ServerToolUse { id, name, input } => Content::ServerToolUse { id, name, input },
        })
    }
}

/// Decode a `content` slot that may be a bare string, a single content object,
/// or a sequence of content — all into the canonical `Vec<Content>`.
pub(crate) fn de_content_seq<'de, D>(d: D) -> Result<Vec<Content>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ContentSeq;
    impl<'de> Visitor<'de> for ContentSeq {
        type Value = Vec<Content>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("content text, a content object, or a sequence of content")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![Content::Text(v.to_owned())])
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            Ok(vec![Content::deserialize(
                de::value::MapAccessDeserializer::new(map),
            )?])
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }
    d.deserialize_any(ContentSeq)
}
