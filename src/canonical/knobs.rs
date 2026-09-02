//! The LIFTED KNOBS (architecture.md §3.1): the closed value vocabularies a
//! [`CanonicalRequest`](super::CanonicalRequest) field carries because every dialect
//! names the same intent under an irreconcilable spelling, so `extra` — a flat
//! top-level valve carrying exactly ONE spelling — cannot express them portably.
//!
//! Each type is the ONE home for its intent plus the SHARED per-family spelling
//! tables the encoders read (`ReasoningEffort::budget()` for the budget dialects,
//! `ServiceTier::anthropic()` for Anthropic's asymmetric lane pair); the dialect
//! chooses which table it needs, and "how big is `medium`?" has one answer. Kept
//! apart from the request/content model in `request` — the model is one concern,
//! its portable knob vocabularies another.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A PORTABLE structured-output intent — one canonical knob every structured-output-
/// capable dialect spells differently, lifted out of `extra` so each adapter owns its
/// projection (the same rule as `ToolChoice`/`reasoning`). Internally tagged on `type`
/// (`{"type":"json"}` / `{"type":"json_schema",...}`), so it rides the wire and config
/// the same way. `name`/`strict` feed only the dialects whose wire has them (OpenAI);
/// Anthropic/Google/Ollama read only `schema` (providers.md §6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OutputFormat {
    /// Plain JSON mode: valid JSON with no schema. OpenAI `json_object`, Google
    /// `responseMimeType` alone, Ollama `format:"json"`; Anthropic has no schemaless
    /// mode → a documented narrowing (omit, providers.md §6).
    Json,
    /// JSON constrained to `schema`. `name` labels the schema where the dialect
    /// requires one (OpenAI); `strict` toggles strict adherence where the wire has it.
    JsonSchema {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        schema: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
}

/// A PORTABLE reasoning-effort intent — one canonical knob every reasoning-capable
/// dialect spells differently, lifted out of `extra` so each adapter owns its
/// projection (the same rule as `ToolChoice`/`parallel_tool_calls`). serde lowercase,
/// so `"low"`/`"medium"`/`"high"` on the wire and in config (providers.md §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// The string spelling for the dialects that take an effort string (OpenAI
    /// Responses `reasoning.effort`, OpenAI Chat `reasoning_effort`).
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }

    /// The SHARED effort→thinking-token-budget table (providers.md §6) for the
    /// budget dialects (Anthropic `thinking.budget_tokens`, Google `thinkingBudget`).
    /// `Low` is the Anthropic minimum (1024), so every rung clears the floor.
    pub fn budget(self) -> u32 {
        match self {
            ReasoningEffort::Low => 1024,
            ReasoningEffort::Medium => 8192,
            ReasoningEffort::High => 24576,
        }
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = ();
    /// Parse the `low|medium|high` spelling (CLI `--reasoning`, `BRAZEN_REASONING`);
    /// `Err(())` for anything else, lifted to a usage/`BadValue` error by the caller.
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "low" => Ok(ReasoningEffort::Low),
            "medium" => Ok(ReasoningEffort::Medium),
            "high" => Ok(ReasoningEffort::High),
            _ => Err(()),
        }
    }
}

/// A PORTABLE PROCESSING-LANE intent — the FIFTH lifted knob (providers.md §6.2):
/// spend the provider's priority lane on this request, or demand the standard one.
/// An ENUM, not a bool: "which lane" is a value, and a bool would be the lossy
/// "is this fact present" projection of it (AGENTS.md) — OpenAI speaks
/// `default|flex|priority`, Anthropic `auto|standard_only`, and a further rung
/// (`Flex`) is additive later under `#[non_exhaustive]`. serde lowercase, so
/// `"priority"`/`"standard"` on the wire and in config; `None` = absent, the key
/// omitted and the provider's own default lane taken (the empty-set path).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ServiceTier {
    /// Spend the priority/fast-token lane where the account has one.
    Priority,
    /// Demand the standard lane — never the priority one.
    Standard,
}

impl ServiceTier {
    /// The OpenAI-family `service_tier` spelling, shared by `openai_chat` and
    /// `openai_responses` (providers.md §6.2). `Standard` is OpenAI's own
    /// `"default"` — the lane's name there, not a second concept.
    pub fn openai(self) -> &'static str {
        match self {
            ServiceTier::Priority => "priority",
            ServiceTier::Standard => "default",
        }
    }

    /// The Anthropic `service_tier` spelling — and the ASYMMETRY (providers.md
    /// §6.2): Anthropic has no request-side priority DEMAND (priority is org
    /// provisioning), so the priority intent is `"auto"`, the value that spends
    /// provisioned priority capacity and falls back to standard. `Standard` is
    /// `"standard_only"`, the explicit refusal of that fallback.
    pub fn anthropic(self) -> &'static str {
        match self {
            ServiceTier::Priority => "auto",
            ServiceTier::Standard => "standard_only",
        }
    }
}

impl std::str::FromStr for ServiceTier {
    type Err = ();
    /// Parse the `priority|standard` spelling (CLI `--tier`, `BRAZEN_TIER`);
    /// `Err(())` for anything else, lifted to a usage/`BadValue` error by the caller.
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "priority" => Ok(ServiceTier::Priority),
            "standard" => Ok(ServiceTier::Standard),
            _ => Err(()),
        }
    }
}
