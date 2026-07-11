//! The one config schema (config §2). Flags, env, file, and embedded defaults
//! are four instances of `PartialConfig`, every field `Option`, every provider
//! entry sparse — `None` is the identity of `Option::or`, so a missing layer
//! contributes nothing and "is this set?" needs no second flag (config §2.1).
//! `or` is the single associative fold step, identical for scalars and the
//! provider table (config §3.1, §3.2). The sparse provider row lives in [`row`];
//! the custom `Deserialize` — the one array-of-tables (`[[provider]]`) ⇄ keyed-map
//! seam (config §2.2) — lives in the sibling `partial_de`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::canonical::{Content, ReasoningEffort};
use crate::store::Secret;

mod ingress;
mod row;

pub use ingress::{LossyMode, PartialIngress};
pub use row::PartialProvider;

use ingress::or_ingress;

/// The output projection (arch §5.1): `--text` default, `--json` → `Ndjson`,
/// `--raw` → `Raw`. The single enum behind both `PartialConfig.output` and
/// `ResolvedConfig.output` — one home for "which projection" (config §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutMode {
    Text,
    Ndjson,
    Raw,
}

impl OutMode {
    /// Parse a config/env spelling (`text`/`ndjson`/`raw`) — `None` for an
    /// unrecognized value, lifted to a `BadValue` by the caller (config §7).
    pub fn parse(s: &str) -> Option<OutMode> {
        match s {
            "text" => Some(OutMode::Text),
            "ndjson" => Some(OutMode::Ndjson),
            "raw" => Some(OutMode::Raw),
            _ => None,
        }
    }
}

/// The one config type (config §2). Built four times — flags, env, file,
/// defaults — and folded under `or`. `provider` is the selected provider name;
/// `providers` is the sparse row table (wire key `[[provider]]`, §2.2).
#[derive(Default, Clone, Debug, PartialEq)]
pub struct PartialConfig {
    pub provider: Option<String>,
    /// The zero-config DEFAULT provider: the name of the FIRST `[[provider]]` row
    /// declared in this layer (config-file order — config §4.3). The `providers`
    /// `BTreeMap` discards declaration order, so this carries the one fact the no-
    /// model fallback needs (AGENTS.md "carry the fact"). NOT user-written and
    /// distinct from `provider`: the selector FORCES a row (and overrides model
    /// routing); this only breaks the tie when there is no selector AND no model.
    /// The fold's `.or()` makes a user file's first row outrank `defaults`', so a
    /// configured first provider beats the built-in `anthropic`.
    pub default_provider: Option<String>,
    pub model: Option<String>,
    /// `--base-url`/`BRAZEN_BASE_URL`/top-level `base_url`: a HOST override that
    /// replaces the RESOLVED row's `base_url` at resolve (config §4.5) — same
    /// provider, different endpoint (a local proxy, mock server, vLLM, tenant
    /// gateway). ONE more top-level scalar folded flag>env>file like `model`,
    /// then laid over the routed row (`self.base_url.or(row.base_url)`), so the
    /// full precedence is flag>env>file-scalar>row. It does NOT create a row —
    /// protocol/auth/api_header stay the resolved row's. DISTINCT from the row's
    /// own `base_url` (a `[[provider]]` field, [`PartialProvider`]): this is the
    /// bare top-level key an embedding harness sets without writing a temp file.
    pub base_url: Option<String>,
    pub api_key: Option<Secret>,
    pub output: Option<OutMode>,
    /// The `--raw` INPUT-axis override (arch §5.4, §5.10.2): the directional split
    /// decouples request-rawness from response-rawness. `output == Raw` is the OUTPUT
    /// axis (the `RawSink`); this is the INPUT axis (send the stdin body verbatim, skip
    /// the constructor+encode). `--raw=in` sets `Some(true)`, `--raw=out` `Some(false)`;
    /// bare `--raw`/`--raw=both` leave it `None` so it DERIVES from the final `output`
    /// (`raw_in = raw_in.unwrap_or(output == Raw)`, in `run`). CLI-only — env/file carry
    /// no direction spelling (a file's `output = "raw"` therefore means BOTH), so this is
    /// never serialized by `--dump-config`; the `output` key already carries the raw fact.
    pub raw_in: Option<bool>,
    /// `--thinking`: emit reasoning before the answer under the text projection
    /// (arch §5.3). A flag on text mode, not a fourth `OutMode` — inert outside it.
    pub thinking: Option<bool>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    /// `--reasoning`/`BRAZEN_REASONING`/file `reasoning = "high"`: the portable
    /// effort knob (arch §3.1, §5.3). A typed gen field folded flag>env>file like
    /// the rest; NOT a `body_defaults` gen scalar — the exact-budget escape hatch
    /// stays the row's raw `body_defaults` object (config §4.1).
    pub reasoning: Option<ReasoningEffort>,
    pub stream: Option<bool>,
    /// The per-request transport SILENCE budget in WHOLE SECONDS (config §4.3,
    /// arch §13.15): abort when the upstream sends no bytes for this long, applied
    /// per phase (connect / response-headers / inter-chunk). ONE value — resolution
    /// fans it onto the three ureq budgets at the seam; NOT a wall-clock total.
    /// `data/defaults.toml` carries the floor; `None` defers like any scalar.
    pub timeout: Option<u64>,
    /// The leading, config-/flag-/file-sourced system prompt (arch §3.1, §4.4,
    /// Decision 10): the ergonomic "data transported by bz", filled into a request
    /// that omits its own `system`. Distinct from a `Role::System` transcript
    /// message — position is the distinguishing fact, not a second home.
    pub system: Option<Vec<Content>>,
    pub providers: BTreeMap<String, PartialProvider>,
    /// The `[ingress]` table (ingress §6): the masquerade listener's one config
    /// surface, a top-level SIBLING of `[[provider]]`, `deny_unknown_fields`
    /// like a row. Sparse and optional so it folds like everything else; a
    /// missing table is the identity, and only a serve/ingress path resolves
    /// (and validates) it — `resolve_ingress`, never `into_resolved`.
    pub ingress: Option<PartialIngress>,
    pub extra: Map<String, Value>,
}

impl PartialConfig {
    /// The fold step: `self` outranks `other`. Every scalar is `Option::or`;
    /// the provider table merges per-key, per-field; the `extra` map lets the
    /// higher-precedence key win. `or` is associative, so the four-layer fold
    /// needs no parenthesization (config §3.1).
    pub fn or(self, other: PartialConfig) -> PartialConfig {
        PartialConfig {
            provider: self.provider.or(other.provider),
            default_provider: self.default_provider.or(other.default_provider),
            model: self.model.or(other.model),
            base_url: self.base_url.or(other.base_url),
            api_key: self.api_key.or(other.api_key),
            output: self.output.or(other.output),
            raw_in: self.raw_in.or(other.raw_in),
            thinking: self.thinking.or(other.thinking),
            max_tokens: self.max_tokens.or(other.max_tokens),
            temperature: self.temperature.or(other.temperature),
            top_p: self.top_p.or(other.top_p),
            reasoning: self.reasoning.or(other.reasoning),
            stream: self.stream.or(other.stream),
            timeout: self.timeout.or(other.timeout),
            system: self.system.or(other.system),
            providers: merge_providers(self.providers, other.providers),
            ingress: or_ingress(self.ingress, other.ingress),
            extra: or_map(self.extra, other.extra),
        }
    }
}

/// Union of keys; a key in both layers merges field-by-field under the same
/// `or` (config §3.2) — the SAME mechanism that folds scalars, no second
/// merge algorithm.
fn merge_providers(
    mut hi: BTreeMap<String, PartialProvider>,
    lo: BTreeMap<String, PartialProvider>,
) -> BTreeMap<String, PartialProvider> {
    for (key, lo_row) in lo {
        let merged = match hi.remove(&key) {
            Some(hi_row) => hi_row.or(lo_row),
            None => lo_row,
        };
        hi.insert(key, merged);
    }
    hi
}

/// The `extra` valve folds like everything else: the higher-precedence key
/// wins, a key only in the lower layer passes through. Shared by the top-level
/// `extra`, a row's `body_defaults`, and the resolve-time merge of a row's
/// non-gen `body_defaults` over the top-level `extra` (config §3.2, §4.1).
pub(crate) fn or_map(mut hi: Map<String, Value>, lo: Map<String, Value>) -> Map<String, Value> {
    for (key, value) in lo {
        hi.entry(key).or_insert(value);
    }
    hi
}
