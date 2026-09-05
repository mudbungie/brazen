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
    /// `id` is the OpenAI Responses reasoning-item id (`rs_…`), surfaced at block
    /// open so a `--json` harness can rebuild the item for replay; `None` for
    /// Anthropic/Google (no reasoning-item id). Serializes `{"thinking":{}}` when
    /// `None` — byte-identical to the pre-reasoning-round-trip shape (bl-61a9).
    Thinking {
        id: Option<String>,
    },
    /// The Anthropic opaque blob, present AT block open (the wire delivers it on
    /// the block start, mirroring `ServerToolResult`'s inline content — no delta
    /// follows), so it round-trips through the decoded stream (bl-61a9).
    RedactedThinking {
        data: String,
    },
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
    /// The opaque signature for the block at this index (bl-61a9): the Anthropic
    /// thinking `signature_delta` (folds to `Content::Thinking.signature`) AND the
    /// Google `thoughtSignature` on a `functionCall` part (folds to
    /// `Content::ToolUse.signature`) — ONE grain, "the signature for block N".
    /// Arrives in wire order, just before the block's stop.
    SignatureDelta(String),
    /// The OpenAI Responses reasoning `encrypted_content` (bl-61a9): a close-
    /// adjacent opaque blob folding to `Content::Thinking.encrypted_content`,
    /// emitted just before the reasoning block's stop (the wire reveals it on the
    /// `output_item.done`). A Delta, not a `ContentStop` field — the terminator
    /// stays a pure, uniform `{index}` for every block kind.
    EncryptedReasoningDelta(String),
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
    /// The call's WHOLE prompt in tokens, cached slices included — the one counter
    /// whose meaning does not depend on which provider answered (§3.2). The four
    /// counters above are each provider's own number, and the providers disagree
    /// about whether the cached slice sits INSIDE the prompt counter (OpenAI chat,
    /// OpenAI Responses, Google: documented as contained) or BESIDE it (Anthropic:
    /// documented as "tokens which were not read from or used to create a cache").
    /// So `input + output + cache_read + cache_write` is right on one dialect and
    /// double-bills the cached slice on the others, growing with the hit rate — worst
    /// exactly where a long conversation is cheapest (bl-d192). The decoder knows the
    /// shape, so it answers here once rather than leaving every consumer to learn the
    /// protocol brazen exists to hide: this call consumed `input_total_tokens +
    /// output_tokens`, everywhere.
    ///
    /// Equal to `input_tokens` wherever the provider's prompt counter is already the
    /// total; that coincidence is those providers' accounting, not this field's
    /// definition. `None` exactly when `input_tokens` is — absent stays absent, never
    /// a fabricated `0` (§3.2), and a partial event that reports only `output_tokens`
    /// (Anthropic's `message_delta`) leaves it `None` rather than claiming a prompt of
    /// zero. Merge a stream's usage events per FIELD, last-wins, then add.
    pub input_total_tokens: Option<u32>,
    /// The resolved model's context window (input token limit) — the DENOMINATOR
    /// for the counters above, carried in-band so a harness that makes no
    /// `--list-models` call still learns it (model-discovery §3, §5.5). NOT a
    /// counter and never wire-served: no provider reports it on a generation
    /// response, so every decoder leaves it `None` and the ONE stamp site
    /// (`run::drive::canonical_events`) carries it off the resolved model row —
    /// the same carry-the-fact rule as the 404 hint and `Retry-After`.
    /// `None` when the row does not state one — absent stays absent, never a
    /// fabricated number (the Usage zero-vs-unknown principle applied to a
    /// capability fact). Unlike the four counters (whose `null` says "this
    /// provider did not report it for THIS call"), it is `serde(default)` +
    /// `skip_serializing_if`, the grows-only shape `Model`'s metadata trio
    /// already uses: a window-less stream serializes byte-identically to the
    /// pre-window event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

impl Usage {
    /// Seal [`Usage::input_total_tokens`] — the ONE home of the containment rule
    /// (§3.2), called by every decoder as it builds the event. `cache_outside_input`
    /// is the dialect's documented accounting: `true` where the cached/written slices
    /// sit BESIDE the prompt counter and must be added back (Anthropic), `false` where
    /// the prompt counter already contains them (OpenAI chat, OpenAI Responses,
    /// Google) or where no cache counter exists at all (Ollama — the two formulas
    /// coincide on the empty case, so it is the general path, not a third rule).
    pub(crate) fn with_input_total(mut self, cache_outside_input: bool) -> Self {
        self.input_total_tokens = match (self.input_tokens, cache_outside_input) {
            (Some(n), true) => Some(
                n.saturating_add(self.cache_read_tokens.unwrap_or(0))
                    .saturating_add(self.cache_write_tokens.unwrap_or(0)),
            ),
            (n, _) => n,
        };
        self
    }
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
