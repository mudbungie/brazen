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
    /// Portable STRUCTURED-OUTPUT intent (architecture.md §3.1): a canonical
    /// JSON-mode / JSON-schema request each protocol projects to its native
    /// structured-output shape in `encode` — the FOURTH lifted known knob (after
    /// `parallel_tool_calls`, `ToolChoice`, `reasoning`), NOT an `extra` key,
    /// because every dialect names the same idea under an irreconcilable spelling
    /// (OpenAI chat `response_format`, OpenAI Responses `text.format`, Google
    /// `generationConfig.responseMimeType`/`responseSchema`, Ollama `format`,
    /// Anthropic `output_config.format`). `None` = plain text (the empty-set path).
    /// The typed knob wins over a same-named `body_defaults`/`extra` key through
    /// every encoder's one `extra` fold (providers.md §6). A backend that rejects
    /// it lists the canonical key `output` in `unsupported_body_keys` (config §4.1.1).
    #[serde(default)]
    pub output: Option<OutputFormat>,
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
/// signatures and `RedactedThinking` data round-trip verbatim (load-bearing), and
/// so do the two opaque server-tool variants (CR-4): provider-executed blocks are
/// carried untouched, mirroring the `RedactedThinking` rule.
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
        /// Google `thoughtSignature` for this tool call — LOAD-BEARING for Gemini
        /// 2.5 multi-turn function calling (the API 400s if it is dropped on
        /// replay). `None` for dialects without it (Anthropic/OpenAI). Folded from
        /// a `Delta::SignatureDelta` on the tool block (bl-61a9).
        signature: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<Content>,
        is_error: bool,
    },
    Thinking {
        text: String,
        /// Anthropic thinking `signature` (the API 400s on an altered/absent one).
        signature: Option<String>,
        /// OpenAI Responses reasoning-item id (`rs_…`), echoed back on replay.
        id: Option<String>,
        /// OpenAI Responses `encrypted_content` — the encrypted reasoning payload
        /// for stateless (`store:false`) replay. `None` for dialects without it
        /// (bl-61a9).
        encrypted_content: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    /// Anthropic server-tool invocation (web_search etc.), opaque (CR-4). Echoed
    /// back VERBATIM on replay; never folded into `ToolUse`. id uses `srvtoolu_`.
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Server-tool RESULT, opaque. `kind` IS the wire tag (`web_search_tool_result`,
    /// `code_execution_tool_result`, …) — an open set carried as data, re-emitted
    /// verbatim on encode. `content` is the untouched provider payload: array of
    /// results on success, or `{type:*_error,...}` object on failure. Echoed back
    /// VERBATIM; NEVER a client `tool_result`.
    ServerToolResult {
        kind: String,
        tool_use_id: String,
        content: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

/// A declared tool — an OPEN SET (brazen enumerates none, registers none). The
/// enum distinguishes only NORMALIZE vs CARRY, and the harness declares which by
/// the shape it hands over: a wire object with no `type` key is `Custom`, one with
/// a `type` key is `Provider` (hand-rolled serde in `request_de`, keyed on `type`).
#[derive(Clone, Debug, PartialEq)] // hand-rolled serde in request_de, keyed on `type`
pub enum Tool {
    /// Caller-defined: brazen understands the STRUCTURE (name, description, JSON
    /// Schema) and PROJECTS it across dialects (Anthropic `input_schema` vs OpenAI
    /// `function.parameters`) — that projection is the whole value.
    Custom {
        name: String,
        description: Option<String>,
        input_schema: Value,
        /// OpenAI-style STRICT function calling (structured-output's per-tool sibling,
        /// architecture.md §3.1): `Some(true)` constrains the tool's `input` to the
        /// schema. A lifted known knob — nested inside the per-tool object each dialect
        /// spells differently (OpenAI chat `function.strict`, Responses flat `strict`,
        /// Anthropic tool `strict`), so `extra` cannot reach it. `None` = provider
        /// default; Google/Ollama LACK the field → a documented narrowing (providers §6).
        strict: Option<bool>,
    },
    /// Provider-typed (Anthropic-schema client tools bash/computer/… AND server
    /// tools web_search/…): opaque `kind` (wire `type`, e.g. `web_search_20250305`)
    /// plus config, passed through to the routed provider verbatim. brazen has no
    /// opinion on `kind`; the provider is the authority (a bad one → provider 400).
    Provider {
        kind: String,
        name: String,
        config: Map<String, Value>,
    },
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
