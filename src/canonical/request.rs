//! The canonical request model (§3.1): one authoritative request type every
//! protocol projects to and from. No IO. `Content` uses a custom serde repr
//! (CR-4) so a bare wire string (`"hi"`) and a `{"type":…}` object both decode
//! to it, and `content` fields accept a string, one object, or a sequence — that
//! wire projection lives in the sibling [`request_de`](super::request_de).

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
    /// May the model emit tool calls in parallel? `None` = provider default.
    /// A lifted known knob (architecture.md §3.1): both providers express it under
    /// different spellings (OpenAI top-level `parallel_tool_calls`, Anthropic nested
    /// `tool_choice.disable_parallel_tool_use`), so each adapter owns its projection.
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
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
    #[serde(deserialize_with = "crate::canonical::request_de::de_content_seq")]
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
