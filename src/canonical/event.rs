//! The canonical streaming event taxonomy (§3.2): the one vocabulary every
//! provider response folds into. No IO; the serde reprs are byte-identical to
//! the §5.2 NDJSON wire sample (`Event` keeps `"type"` internal tagging;
//! `ContentKind`/`Delta` are externally tagged per CR-4).

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::canonical::error::CanonicalError;
use crate::canonical::request::Role;

/// Event-schema version stamped into the first `MessageStart` (§3.2). The one
/// handshake a harness pins to; a backward-incompatible change to the `Event`
/// vocabulary bumps it.
pub const EVENT_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    MessageStart {
        v: u8,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        model: Option<String>,
        role: Role,
    },
    ContentStart {
        index: u32,
        kind: ContentKind,
    },
    ContentDelta {
        index: u32,
        delta: Delta,
    },
    ContentStop {
        index: u32,
    },
    Usage(Usage),
    Finish {
        #[serde(flatten)]
        reason: FinishReason,
    },
    Error(CanonicalError),
    /// Only under `--raw`; written verbatim by the raw sink, never serialized.
    #[serde(skip)]
    Raw(Vec<u8>),
    /// THE provider-agnostic terminator.
    End,
}

impl Event {
    /// Build the opening event, stamping the schema version from the single
    /// `EVENT_SCHEMA_VERSION` const so adapters never retype the number (§3.2).
    pub fn message_start(id: Option<String>, model: Option<String>, role: Role) -> Event {
        Event::MessageStart {
            v: EVENT_SCHEMA_VERSION,
            id,
            model,
            role,
        }
    }
}

/// What kind of content block is opening (§3.2). Externally tagged so it
/// renders `{"text":{}}` / `{"tool_use":{…}}` exactly as the §5.2 sample shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text {},
    ToolUse { id: String, name: String },
    Thinking {},
    RedactedThinking {},
}

/// A streamed content fragment (§3.2). Externally tagged so a newtype variant
/// renders `{"text_delta":"Hel"}`. Tool arguments ride `JsonDelta` as text
/// fragments, never a parsed `Value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    TextDelta(String),
    JsonDelta(String),
    ThinkingDelta(String),
}

/// Token accounting (§3.2). Every field is `Option`: a provider that never
/// reports a counter leaves it `None` (`0` would be a lie), never fabricated.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: Option<u32>,
    pub output: Option<u32>,
    pub cache_read: Option<u32>,
    pub cache_write: Option<u32>,
}

/// Why generation stopped (§3.2). Carried flattened into `Event::Finish`, keyed
/// on `reason`. Refusal is a `Finish`, never an `Error`. `Other` preserves any
/// unknown reason string so decode never panics on a new value.
#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    Stop,
    Length,
    ToolUse,
    StopSequence,
    Refusal {
        category: String,
        explanation: Option<String>,
    },
    Pause,
    Other(String),
}

impl Serialize for FinishReason {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        match self {
            FinishReason::Stop => m.serialize_entry("reason", "stop")?,
            FinishReason::Length => m.serialize_entry("reason", "length")?,
            FinishReason::ToolUse => m.serialize_entry("reason", "tool_use")?,
            FinishReason::StopSequence => m.serialize_entry("reason", "stop_sequence")?,
            FinishReason::Pause => m.serialize_entry("reason", "pause")?,
            FinishReason::Refusal {
                category,
                explanation,
            } => {
                m.serialize_entry("reason", "refusal")?;
                m.serialize_entry("category", category)?;
                m.serialize_entry("explanation", explanation)?;
            }
            FinishReason::Other(reason) => m.serialize_entry("reason", reason)?,
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for FinishReason {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            reason: String,
            #[serde(default)]
            category: Option<String>,
            #[serde(default)]
            explanation: Option<String>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(match raw.reason.as_str() {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_use" => FinishReason::ToolUse,
            "stop_sequence" => FinishReason::StopSequence,
            "pause" => FinishReason::Pause,
            "refusal" => FinishReason::Refusal {
                category: raw.category.unwrap_or_default(),
                explanation: raw.explanation,
            },
            _ => FinishReason::Other(raw.reason),
        })
    }
}
