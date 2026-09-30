//! Token accounting (§3.2): the canonical `Usage` counters and the two decoder-side
//! answers to the providers' containment disagreements — the prompt total
//! (`with_input_total`) and the thinking split (`with_thinking`) — plus the one
//! consumer-preference fold (`fold_thinking`). Split from `event` for the line cap;
//! `event::Usage` re-exports it, so every path is unchanged.

use serde::{Deserialize, Serialize};

/// Token accounting (§3.2). Every field is `Option`: a provider that never
/// reports a counter leaves it `None` (`0` would be a lie), never fabricated.
/// Token-explicit names — these count tokens (Anthropic `input_tokens`/…,
/// OpenAI `prompt_tokens`/…) — frozen with the rest of the `v=1` vocabulary.
///
/// `#[non_exhaustive]`: a future counter (e.g. deferred server-tool counts — §3.2) is an additive `v=1` change, never breaking a
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
    /// The tokens the model spent REASONING, where the provider serves that split
    /// (Google `thoughtsTokenCount`, OpenAI chat `completion_tokens_details.
    /// reasoning_tokens`, OpenAI Responses `output_tokens_details.reasoning_tokens`).
    /// Wherever it is `Some`, [`Self::output_tokens`] EXCLUDES it: the providers
    /// disagree about containment exactly as they do for the cached prompt slice
    /// (Google serves the answer beside the thoughts; OpenAI serves a total that
    /// contains them), so the decoder answers once ([`Self::with_thinking`]) and the
    /// two counters never shadow each other (bl-2042).
    ///
    /// `None` where the provider serves no split (Anthropic, Ollama): unknown, never a
    /// fabricated `0` — and there `output_tokens` is the provider's number as served,
    /// thinking INCLUDED, because nothing can separate it. The industry convention
    /// (thinking inside output) is the consumer's `usage_fold_thinking` knob
    /// ([`Self::fold_thinking`]), never a decoder fact. Grows-only like
    /// `context_window`: omitted when `None`, so a split-less stream is byte-identical
    /// to the pre-split event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u32>,
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

    /// Seal [`Usage::thinking_tokens`] — the ONE home of the output-side containment
    /// rule (§3.2), the sibling of [`Self::with_input_total`]. `thinking` is the served
    /// split (`None` when the wire carried none); `output_contains_thinking` is the
    /// dialect's documented accounting: `true` where the served output counter already
    /// CONTAINS the split (OpenAI chat, OpenAI Responses — subtract it, leaving the
    /// answer), `false` where it sits BESIDE it (Google). A `None` split is the empty
    /// case of both rules: nothing to subtract, `thinking_tokens` stays unknown.
    pub(crate) fn with_thinking(
        mut self,
        thinking: Option<u32>,
        output_contains_thinking: bool,
    ) -> Self {
        if let (Some(out), Some(t), true) = (self.output_tokens, thinking, output_contains_thinking)
        {
            self.output_tokens = Some(out.saturating_sub(t));
        }
        self.thinking_tokens = thinking;
        self
    }

    /// The `usage_fold_thinking` projection (config §2): the industry convention, where
    /// `output_tokens` counts reasoning too. Folds `thinking_tokens` INTO
    /// `output_tokens` where both are known and leaves `thinking_tokens` reported; an
    /// unknown split (Anthropic, Ollama — already inside the served number) or an
    /// absent output is the identity. A consumer preference, applied once at the usage
    /// stamp site (`run::drive`), never by a decoder.
    pub fn fold_thinking(mut self) -> Self {
        if let (Some(out), Some(t)) = (self.output_tokens, self.thinking_tokens) {
            self.output_tokens = Some(out.saturating_add(t));
        }
        self
    }
}
