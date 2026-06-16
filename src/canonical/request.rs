//! The canonical request model (§3.1): one authoritative request type every
//! protocol projects to and from. No IO. `Content` uses a custom serde repr
//! (CR-4) so a bare wire string (`"hi"`) and a `{"type":…}` object both decode
//! to it, and `content` fields accept a string, one object, or a sequence.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The single canonical request. A field set on the wire is used as-is; a field
/// it omits defaults (`getConfigValue` fills it later — §6.1). `extra` is the
/// long-tail valve: an unmodelled top-level key is forwarded verbatim.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRequest {
    /// Empty = absent: a request may omit `model` and let config supply it
    /// (`fill_absent`, §4.3/§4.4). An empty string and a missing key are the same
    /// "no model" fact — never two cases.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub system: Option<Vec<Content>>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A transcript message. `content` is ALWAYS a `Vec<Content>`; a bare wire
/// string decodes to `vec![Text(..)]` (the string-vs-list distinction dies at
/// decode, never a downstream branch).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(deserialize_with = "de_content_seq")]
    pub content: Vec<Content>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A piece of content. `Text` is expressible both as a bare string and as a
/// `{"type":"text",…}` object; the other variants are tagged objects. `Thinking`
/// signatures and `RedactedThinking` data round-trip verbatim (load-bearing).
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<Content>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: Value,
}

/// All four tool-use intents, lifted explicitly rather than left in `extra`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    Any,
    Tool {
        name: String,
    },
    None,
}

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
                input: &'a Value,
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
                input: Value,
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
fn de_content_seq<'de, D>(d: D) -> Result<Vec<Content>, D::Error>
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
