//! The canonical streaming event taxonomy (§3.2): the one vocabulary every
//! provider response folds into. No IO; the serde reprs are byte-identical to
//! the §5.2 NDJSON wire sample (`Event` keeps `"type"` internal tagging;
//! `ContentKind`/`Delta` are externally tagged per CR-4).
//!
//! **The `v=1` forward-compat contract (§3.2).** Within a fixed
//! `EVENT_SCHEMA_VERSION` the vocabulary only GROWS: a consumer MUST tolerate an
//! unknown event `type`, content `kind`, or `delta` variant — and unknown object
//! fields — by ignoring it, so a new additive kind/event never breaks a pinned
//! consumer. Every open enum here carries an `Other` catch-all (the general
//! path; `FinishReason::Other` is the same rule, not a special case) and is
//! `#[non_exhaustive]` (a new Rust variant is non-breaking too). `v` bumps ONLY
//! for a removal, rename, or semantic change — never for an addition.

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::canonical::error::CanonicalError;
use crate::canonical::request::Role;

/// Event-schema version stamped into the first `MessageStart` (§3.2). The one
/// handshake a harness pins to; a backward-incompatible change to the `Event`
/// vocabulary bumps it (an additive kind/event does NOT — see the module doc).
pub const EVENT_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// Forward-compat (§3.2 `v=1` contract): an event `type` this build does not
    /// model decodes here instead of erroring. `#[serde(other)]` is internal
    /// tagging's skip path — the payload drops, a pinned consumer ignores it.
    #[serde(other)]
    Other,
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
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ContentKind {
    Text {},
    ToolUse {
        id: String,
        name: String,
    },
    Thinking {},
    RedactedThinking {},
    /// Forward-compat: an unknown externally-tagged `kind` rides here verbatim
    /// (the whole `{tag: body}` object) so a pinned consumer passes it through.
    Other(Value),
}

/// A streamed content fragment (§3.2). Externally tagged so a newtype variant
/// renders `{"text_delta":"Hel"}`. Tool arguments ride `JsonDelta` as text
/// fragments, never a parsed `Value`.
// The `*Delta` variant names mirror the wire tags the manual `Serialize`/`Deserialize`
// below emit (`text_delta`/`json_delta`/`thinking_delta`), so the `Delta` suffix is
// intentional, not a naming slip. `enum_variant_names` only began firing once `Delta`
// left the public surface (arch §9.8) — clippy exempts exported API from it — so the
// allow records the deliberate, wire-tied names.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
#[allow(clippy::enum_variant_names)]
pub enum Delta {
    TextDelta(String),
    JsonDelta(String),
    ThinkingDelta(String),
    /// Forward-compat: an unknown `delta` rides here verbatim (the whole
    /// `{tag: body}` object) so a pinned consumer passes it through.
    Other(Value),
}

/// Serialize a single externally-tagged entry `{tag: body}` — the shared shape
/// of every `ContentKind`/`Delta` variant.
fn tagged<S, T>(s: S, tag: &str, body: &T) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize + ?Sized,
{
    let mut m = s.serialize_map(Some(1))?;
    m.serialize_entry(tag, body)?;
    m.end()
}

/// The externally-tagged body of `ContentKind::{Text,Thinking,RedactedThinking}`
/// — an empty object `{}`, never the `null` a unit struct would render.
struct EmptyBody;
impl Serialize for EmptyBody {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_map(Some(0))?.end()
    }
}

#[derive(Serialize)]
struct ToolUseBody<'a> {
    id: &'a String,
    name: &'a String,
}

/// The single key of an externally-tagged object (`None` if it is not a
/// one-key object), used to dispatch the tag on decode.
fn tag_of(v: &Value) -> Option<&str> {
    v.as_object()?.keys().next().map(String::as_str)
}

/// A string field of `v` (the empty string if absent/non-string — known
/// variants always carry it; the lenient path only meets our own output).
fn str_at(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

impl Serialize for ContentKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ContentKind::Text {} => tagged(s, "text", &EmptyBody),
            ContentKind::ToolUse { id, name } => tagged(s, "tool_use", &ToolUseBody { id, name }),
            ContentKind::Thinking {} => tagged(s, "thinking", &EmptyBody),
            ContentKind::RedactedThinking {} => tagged(s, "redacted_thinking", &EmptyBody),
            ContentKind::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for ContentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match tag_of(&v) {
            Some("text") => ContentKind::Text {},
            Some("tool_use") => ContentKind::ToolUse {
                id: str_at(&v["tool_use"], "id"),
                name: str_at(&v["tool_use"], "name"),
            },
            Some("thinking") => ContentKind::Thinking {},
            Some("redacted_thinking") => ContentKind::RedactedThinking {},
            _ => ContentKind::Other(v),
        })
    }
}

impl Serialize for Delta {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Delta::TextDelta(t) => tagged(s, "text_delta", t),
            Delta::JsonDelta(t) => tagged(s, "json_delta", t),
            Delta::ThinkingDelta(t) => tagged(s, "thinking_delta", t),
            Delta::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Delta {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match tag_of(&v) {
            Some("text_delta") => Delta::TextDelta(str_at(&v, "text_delta")),
            Some("json_delta") => Delta::JsonDelta(str_at(&v, "json_delta")),
            Some("thinking_delta") => Delta::ThinkingDelta(str_at(&v, "thinking_delta")),
            _ => Delta::Other(v),
        })
    }
}

/// Token accounting (§3.2). Every field is `Option`: a provider that never
/// reports a counter leaves it `None` (`0` would be a lie), never fabricated.
/// Token-explicit names — these count tokens (Anthropic `input_tokens`/…,
/// OpenAI `prompt_tokens`/…) — frozen with the rest of the `v=1` vocabulary.
///
/// `#[non_exhaustive]`: a future counter (e.g. `reasoning_tokens`, deferred
/// server-tool counts — §3.2) is an additive `v=1` change, never breaking a
/// downstream reader. Out-of-crate construction is `Usage::default()` then field
/// assignment (the fields stay `pub`); the struct literal is in-crate-only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
}

/// Why generation stopped (§3.2). Carried flattened into `Event::Finish`, keyed
/// on `reason`. Refusal is a `Finish`, never an `Error`. `Other` preserves any
/// unknown reason string so decode never panics on a new value.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
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
