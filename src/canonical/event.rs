//! The canonical streaming event taxonomy (§3.2): the one vocabulary every
//! provider response folds into. No IO; the serde reprs are byte-identical to
//! the §5.2 NDJSON wire sample (`Event` keeps `"type"` internal tagging;
//! `ContentKind`/`Delta` are externally tagged per CR-4 — their hand-rolled
//! impls live in the sibling `event_serde`, mirroring request.rs/request_de.rs).
//!
//! **The `v=1` forward-compat contract (§3.2).** Within a fixed
//! `EVENT_SCHEMA_VERSION` the vocabulary only GROWS: a consumer MUST tolerate an
//! unknown event `type`, content `kind`, or `delta` variant — and unknown object
//! fields — by ignoring it, so a new additive kind/event never breaks a pinned
//! consumer. Every open enum here carries an `Other` catch-all (the general
//! path; `FinishReason::Other` is the same rule, not a special case) and is
//! `#[non_exhaustive]` (a new Rust variant is non-breaking too). `v` bumps ONLY
//! for a removal, rename, or semantic change — never for an addition.

use serde::{Deserialize, Serialize};
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
    /// Opaque server-tool invocation (CR-4). Streams start+json_delta+stop like ToolUse.
    ServerToolUse {
        id: String,
        name: String,
    },
    /// Opaque server-tool RESULT. `kind` is the verbatim wire tag (open set); the full
    /// `content` arrives INLINE at content_block_start (no deltas).
    ServerToolResult {
        kind: String,
        tool_use_id: String,
        content: Value,
    },
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
