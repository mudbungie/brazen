//! The wire repr of [`Content`](super::Content) (CR-4): a custom serde pair so a
//! bare string (`"hi"`) and a `{"type":…}` object both decode to it, plus
//! `de_content_seq` for a `content` slot that accepts a string, one object, or a
//! sequence. Kept apart from the type definitions — the model is one concern, its
//! lossless projection to/from the wire another.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

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
            },
            ToolResult {
                tool_use_id: &'a str,
                content: &'a [Content],
                is_error: bool,
            },
            Thinking {
                text: &'a str,
                signature: &'a Option<String>,
            },
            RedactedThinking {
                data: &'a str,
            },
        }
        let tagged = match self {
            Content::Text(text) => Tagged::Text { text },
            Content::Image { source } => Tagged::Image { source },
            Content::ToolUse { id, name, input } => Tagged::ToolUse { id, name, input },
            Content::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Tagged::ToolResult {
                tool_use_id,
                content,
                is_error: *is_error,
            },
            Content::Thinking { text, signature } => Tagged::Thinking { text, signature },
            Content::RedactedThinking { data } => Tagged::RedactedThinking { data },
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
                signature: Option<String>,
            },
            RedactedThinking {
                data: String,
            },
        }
        // A bare string is `Text`; anything else decodes by its `type` tag.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bare(String),
            Tagged(Tagged),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Bare(text) => Content::Text(text),
            Repr::Tagged(Tagged::Text { text }) => Content::Text(text),
            Repr::Tagged(Tagged::Image { source }) => Content::Image { source },
            Repr::Tagged(Tagged::ToolUse { id, name, input }) => {
                Content::ToolUse { id, name, input }
            }
            Repr::Tagged(Tagged::ToolResult {
                tool_use_id,
                content,
                is_error,
            }) => Content::ToolResult {
                tool_use_id,
                content,
                is_error,
            },
            Repr::Tagged(Tagged::Thinking { text, signature }) => {
                Content::Thinking { text, signature }
            }
            Repr::Tagged(Tagged::RedactedThinking { data }) => Content::RedactedThinking { data },
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
