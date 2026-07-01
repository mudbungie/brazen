//! The canonical request model (§3.1): one authoritative request type every
//! protocol projects to and from. No IO. `Content` uses a custom serde repr
//! (CR-4) so a bare wire string (`"hi"`) and a `{"type":…}` object both decode
//! to it, and `content` fields accept a string, one object, or a sequence — that
//! wire projection lives in the sibling `request_de`.

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
    /// Portable reasoning EFFORT (architecture.md §3.1): a canonical user intent
    /// (`low|medium|high`) each protocol maps to its native reasoning shape in
    /// `encode` — a lifted known knob (like `parallel_tool_calls`), NOT an `extra`
    /// key, because the whole point is the canonical→per-protocol mapping. `None` =
    /// no reasoning requested. Exact provider budgets/objects stay reachable via the
    /// row's `body_defaults` escape hatch (config §4.1), which the typed knob wins
    /// over on a same-named key through every encoder's one `extra` fold.
    #[serde(default)]
    pub reasoning: Option<ReasoningEffort>,
    #[serde(default)]
    pub stop: Vec<String>,
    /// Wire-stream the response? `None` = absent, so `fill_absent` supplies it
    /// from config (`--stream`/`BRAZEN_STREAM`/file), like every other gen field;
    /// a request that sets it wins. Encoders read `unwrap_or(false)`. Request-
    /// shaping only — "stream over" is `Event::End`, never this (architecture §3.1).
    #[serde(default)]
    pub stream: Option<bool>,
    /// Anthropic prompt-cache breakpoints (anthropic-messages §2.10). REQUEST-ONLY
    /// structural payload (like `messages`/`tools`): not config-filled, no flag, not
    /// stripped. ONLY the Anthropic encoder projects it to per-block `cache_control`;
    /// every other dialect caches by prompt prefix and ignores it. Empty = no caching
    /// (the general path with empty input — never a branch). Order is significant.
    #[serde(default)]
    pub cache: Vec<CacheBreakpoint>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CanonicalRequest {
    /// Resolve a tool call's function name from its `tool_use_id` (§4.5): scan
    /// every `Content::ToolUse` across messages for the matching `id`. The name is
    /// a fact that lives once, on the originating `ToolUse`; a `ToolResult`
    /// references it by id, so a NAME-keyed dialect (Google `functionResponse`,
    /// Ollama tool message) resolves it here rather than denormalizing a copy onto
    /// `ToolResult` (SSOT). `None` when the call is absent from this request (a bare
    /// tool-result turn) — the name is genuinely not in-band, so the consumer falls
    /// back (Google → the id, Ollama → omit `tool_name`).
    pub fn tool_name(&self, tool_use_id: &str) -> Option<&str> {
        self.messages
            .iter()
            .flat_map(|m| &m.content)
            .find_map(|c| match c {
                Content::ToolUse { id, name, .. } if id == tool_use_id => Some(name.as_str()),
                _ => None,
            })
    }
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
#[non_exhaustive]
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
#[non_exhaustive]
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

/// One prompt-cache breakpoint: WHERE to cut the prefix (`anchor`) and HOW LONG
/// the entry lives (`ttl`). `anchor` is flattened so the wire/config shape is a
/// single flat object: {"anchor":"tools","ttl":"1h"} / {"anchor":"message","index":2}.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    #[serde(flatten)]
    pub anchor: CacheAnchor,
    #[serde(default)]
    pub ttl: CacheTtl,
}

/// The cut point. `Tools`/`System` anchor the whole hoisted block; `Message{index}`
/// anchors a canonical-message index (resolved through the System-hoist skip at
/// encode). snake_case + internal tag `anchor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "anchor", rename_all = "snake_case")]
pub enum CacheAnchor {
    Tools,
    System,
    Message { index: u32 },
}

/// Cache lifetime (anthropic-messages §2.10). `FiveMin` is Anthropic's default and
/// is emitted by OMITTING `ttl`; `OneHour` emits `"ttl":"1h"`. Serde renames are the
/// one home for the `"5m"`/`"1h"` spellings on the canonical (config/wire) surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[default]
    #[serde(rename = "5m")]
    FiveMin,
    #[serde(rename = "1h")]
    OneHour,
}
