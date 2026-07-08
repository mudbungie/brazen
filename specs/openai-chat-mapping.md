# Canonical ⇄ OpenAI `chat/completions` mapping

> **Living document.** Edited like code. This spec is a **lossy projection** onto and back from the canonical model of architecture.md; it MUST NOT contradict the architecture spec. Where the OpenAI `chat/completions` wire cannot express a canonical fact (or vice-versa), this spec raises a **change request** against the architecture spec (§6) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md)

---

## 1. Purpose & Scope

Define, normatively, the `Protocol` implementation registered under `ProtocolId::OpenAiChat` (architecture.md §4.2, §4.4) — the bidirectional lossy projection between the **canonical model** (architecture.md §3, the single source of truth) and the OpenAI **Chat Completions** wire dialect (`POST {base_url}/chat/completions`, SSE streaming).

This spec covers exactly the three methods of the `Protocol` trait (architecture.md §4.1):

```rust
fn encode(&self, req: &CanonicalRequest, ctx: &ProviderCtx) -> Result<WireRequest, Error>;
fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, Error>;
fn framing(&self) -> Framing;   // == Framing::Sse for this protocol
```

**In scope:** the request body projection (§2), the streaming response → `Vec<Event>` decode and the `DecodeState` it threads (§3), provider-error parsing + the HTTP-status→exit-code table (§4), the golden fixtures this protocol contributes and its half of the cross-check (§5), edge cases and change requests (§6).

**Out of scope (owned by the architecture spec or other specs):** auth headers (set by `Auth::apply`, architecture.md §4.1, §4.5 — `encode` sets only body + non-auth headers); the SSE framing mechanics, the error-body→`Frame` plumbing, and `DecodeState`'s buffer (the SSE-decoder spec (planned) — §3 and §4 name the exact decoder contracts they depend on); config/alias resolution and `body_defaults` folding (the config spec, §4.1); the NDJSON `Sink`, `--text`, `--raw`, the exit-code driver loop, premature-EOF handling, and signal handling (architecture.md §5, §8). This protocol is **vendor-blind**: `ProviderCtx` carries no vendor name / `ProtocolId` (architecture.md §4.1). The Chat Completions dialect is shared verbatim by OpenAI, Mistral, local Ollama-in-OpenAI-mode, etc. — those are *rows of data* (architecture.md §4.2), and nothing in this spec may branch on which provider sent the bytes.

### 1.1 Inherited invariants (from the architecture spec — the grading rubric this mapping upholds)

1. `content` is **always** `Vec<Content>`; a bare wire string decodes to `vec![Content::Text(..)]` (architecture.md §3.1). `ToolResult.content` is `Vec<Content>` too.
2. `Role::Tool` is canonical; this adapter owns its own projection onto OpenAI's `role:"tool"` (architecture.md §3.1). The core never branches on tool convention.
3. **Identity precedes content**: emit `ContentStart{index, kind}` (carrying tool `id`/`name`) *before* any `ContentDelta` for that index (architecture.md §3.2). OpenAI gives no `content_block_start`, so this adapter **synthesizes** it.
4. Tool-call arguments stream as `Delta::JsonDelta(String)` fragments — **never parsed mid-stream**; parsed to a `Value` only when folding to `Content::ToolUse` (architecture.md §3.2, §3.6).
5. **Exactly one `Event::End` per response.** `decode` **never emits `End`** — the single terminator is the `sink.write(&Event::End)` the `run` loop appends once after the body iterator drains (architecture.md §4.4). `data: [DONE]` decodes to `[]`; a non-2xx whole-body error (§4.1) and a mid-stream `data: {"error":…}` frame (§4.3) each decode to `[Error(..)]` — none emit `End`. **Two markers set `state.terminated = true`** (§3.6, CR-9, bl-296d): `[DONE]` **and** a non-null `finish_reason` chunk (the latter lets a compat server that closes without `[DONE]` finish cleanly, not premature). This matches the Anthropic messages mapping (anthropic-messages.md) §3.8 — End ownership is the same on both protocols (§3.6).
6. **Refusal is a `Finish{Refusal{..}}`, never an `Error`** (architecture.md §3.2); it arrives HTTP 200 → exit 0. `Error` is its own event, never folded into `Finish` (architecture.md §3.3).
7. `Usage` fields are `Option`; `None` ≠ a fabricated `0` (architecture.md §3.2). Cumulative; emitted when revealed.
8. `decode` is a **pure** state machine over `(frame, &mut DecodeState)`; all cross-frame state lives in `DecodeState`, not in the impl (architecture.md §4.1). Provider-error parsing lives in `decode`; HTTP status is peeked separately for the exit code (architecture.md §8).

### 1.2 The provider row this impl is paired with

For reference (architecture.md §4.2 — this is **data**, not part of the impl):

```toml
[[provider]]
name = "openai"
base_url = "https://api.openai.com/v1"
protocol = "openai_chat"
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
```

The built-in OpenAI row defines **no** `beta_headers` and **no** `body_defaults` (Chat Completions does not require `max_tokens`, so the row pins nothing). `encode` sets only `content-type`; the `Authorization: Bearer` header is set by `BearerAuth::apply` (architecture.md §4.5). `Mistral` and other OpenAI-dialect providers are additional rows pointing at the same `protocol = "openai_chat"` — no code.

---

## 2. REQUEST mapping — canonical → `openai_chat` wire

`encode(req, ctx)` produces the JSON body of `POST {ctx.base_url}/chat/completions`. It sets **only** the body and the `Content-Type` header; **auth headers are set by `Auth::apply`** (architecture.md §4.5). By encode time `req.model` is already the alias-resolved wire id (`ctx.model`), and `req.max_tokens` is already `Some(..)` if this provider's row required one (folded at lowest precedence in config resolution, the config spec (planned)) else `None` (the OpenAI row requires none, so it is normally `None`). `encode` performs **no** alias resolution and **no** max-tokens defaulting.

### 2.1 Top-level body — field-by-field

| Canonical (`CanonicalRequest`) | OpenAI request field | Rule |
|---|---|---|
| `model: String` | `"model"` | `ctx.model` (already wire id). Always present. |
| `system: Option<Vec<Content>>` | leading `messages[0]` `{role:"system"}` | If `Some(non-empty)`, prepend one system message (§2.3). `None` or `Some(vec![])` → no system message (the empty set is not a special case, architecture.md §3.1). |
| `messages: Vec<Message>` | `"messages"` (array, minItems 1) | Each `Message` projected per §2.2, with the synthesized system message prepended. |
| `tools: Vec<Tool>` | `"tools"` | **Omit when empty.** Else array of `{type:"function", function:{name, description?, parameters}}` (§2.5). |
| `tool_choice: ToolChoice` | `"tool_choice"` | Per §2.6. **Omit for `Auto`** (OpenAI's own default); emit explicit value only for `Any`/`None`/`Tool`. |
| `parallel_tool_calls: Option<bool>` | `"parallel_tool_calls"` | `Some(b)`→top-level bool; `None`→omit (OpenAI's default `true` applies). A lifted known knob (architecture.md §3.1); Anthropic nests the same intent in `tool_choice` (anthropic-messages.md §2.7). |
| `max_tokens: Option<u32>` | `"max_tokens"` / `"max_completion_tokens"` | `Some(n)`→`n`; `None`→omit. Key selection: **`"max_completion_tokens"` when `reasoning` is set** (reasoning models reject the deprecated `max_tokens`), else `"max_tokens"` (§2.7). |
| `temperature: Option<f32>` | `"temperature"` | `Some`→value; `None`→omit. **Omitted when `reasoning` is set** — reasoning models 400 on non-default sampling (§2.7). |
| `top_p: Option<f32>` | `"top_p"` | `Some`→value; `None`→omit. **Omitted when `reasoning` is set** (§2.7). |
| `stop: Vec<String>` | `"stop"` | **Omit when empty.** Else emit as an array (always-safe form; do not collapse to a bare string). OpenAI caps at 4; >4 is a provider concern, passed through. |
| `stream: bool` | `"stream"` | The bool. When `true`, also set `stream_options.include_usage = true` (§2.8). |
| `extra: Map<String,Value>` (`#[serde(flatten)]`) | merged into top-level body | The long-tail valve (architecture.md §3.1 — "the long-tail valve **only**"). Carries keys with **no canonical home** (`reasoning_effort`, `seed`, `n`, `logprobs`, `presence_penalty`, `frequency_penalty`, `response_format`, `service_tier`, `max_completion_tokens`, …). §2.1.1. |

`parallel_tool_calls` is now the typed canonical field above (omitted → OpenAI's default `true`). `n`, `seed`, `logprobs`, `presence_penalty`, `frequency_penalty`, `response_format`, `service_tier` have **no canonical home** and reach the wire only via `extra`.

#### 2.1.1 `extra` precedence (single source of truth)

`encode` serializes the typed canonical fields **first**, then folds in `extra` keys that are **not already set by a typed field** — the **typed field wins** (it is the single source of truth; `extra` is the long-tail valve, architecture.md §3.1). `extra` MUST NOT override a field derived from a typed canonical field. This is the **same precedence rule as the Anthropic messages mapping (anthropic-messages.md) §2.1.1** — the two protocol adapters give `extra` identical precedence, and it avoids the `#[serde(flatten)]` duplicate-key hazard. The `max_tokens`-vs-`max_completion_tokens` key-selection is **not** an `extra`-override case: `encode` selects the key from the typed `reasoning` signal (§2.7), and the non-reasoning row-config path resolves at the row/resolution layer.

### 2.2 `messages[]` — per-`Message` projection

A `Message{role, content}` becomes one or more wire messages. The role discriminant:

| `Role` | wire `role` |
|---|---|
| `System` | `"system"` (or `"developer"` if the row opts in, §2.3) |
| `User` | `"user"` |
| `Assistant` | `"assistant"` |
| `Tool` | `"tool"` — **one wire message per `Content::ToolResult`** (§2.4) |

Content-part projection within a message:

| `Content` variant | OpenAI content part / placement |
|---|---|
| `Text(s)` | `{type:"text", text:s}`. A message whose content is a single `Text` MAY be emitted as a bare string (`"content":"…"`) — the common OpenAI shape; the array form is always valid. The adapter SHOULD prefer the bare string for a single text part and the array form when ≥2 parts or any non-text part is present (§6, decided edge case). |
| `Image{source: Base64{media_type,data}}` | `{type:"image_url", image_url:{url:"data:{media_type};base64,{data}"}}` — base64 embedded as a **data-URI string** inside `url`. Chat Completions has **no** structured `{media_type,data}` image source (§6 CR-1). |
| `Image{source: Url{url}}` | `{type:"image_url", image_url:{url}}` |
| `ToolUse{id,name,input}` | an **assistant** `tool_calls[]` entry `{id, type:"function", function:{name, arguments: to_json_string(input)}}`. `arguments` is a **JSON-encoded string**, never a nested object. Collected from the assistant message's content into its `tool_calls` array. |
| `ToolResult{tool_use_id, content, is_error}` | a separate `{role:"tool", tool_call_id: tool_use_id, content:<flattened>}` message (§2.4). |
| `Thinking{text, signature}` | **dropped** — no request representation in Chat Completions (§2.9, §6 CR-2). |
| `RedactedThinking{data}` | **dropped** — no request representation in Chat Completions (Anthropic-only opaque block; §2.9). |

**Assistant message with tool calls.** An assistant `Message` whose content mixes `Text` and `ToolUse` becomes one wire `{role:"assistant"}` with `content` = the text (string or text-parts array, **or omitted entirely when there is no text** — `content` is nullable and MUST NOT be force-emitted as `""` alongside `tool_calls`) and `tool_calls` = the array of projected `ToolUse` entries.

### 2.3 System handling (and `developer`)

`req.system: Option<Vec<Content>>` is projected into a **single leading message** with `role:"system"` (the default) whose content is the concatenated system text. Reasoning models (o1+) replace `system` with `role:"developer"`; this is selected **per row** (an `extra`/config signal, the config spec (planned)), never by branching on a provider name in this impl. Both `system` and `developer` accept **text content parts only** — `req.system` stays a permissive `Vec<Content>` canonically (architecture.md §3.1), so any non-text `Content` in `system` is a runtime degradation surfaced as `Error{kind: ErrorKind::ParseInput}` (→ exit 64) by `encode` before send, since this text-only wire slot cannot represent it (architecture.md §3.1 non-text-slot rejection; §6 CR-5; the parallel of the Anthropic messages mapping's non-text-`system` rejection).

A `Role::System` *message* in `messages` (as opposed to the dedicated `system` field) maps the same way; if both `req.system` and an in-band system message are present, both are emitted in order (the canonical model already permits both; the adapter does not deduplicate).

### 2.4 `Role::Tool` projection (the reframe)

Canonically a `Role::Tool` message carries one or more `Content::ToolResult`. OpenAI keys each tool result by a **single** `tool_call_id`, so **each `ToolResult` becomes its own `{role:"tool"}` message** — they are **not** merged. For a `ToolResult{tool_use_id, content, is_error}`:

- `tool_call_id` ← `tool_use_id`.
- `content` ← `content` flattened to OpenAI tool-message content (string or array of `text` parts only; tool messages accept no `image_url`). `ToolResult.content` stays a permissive `Vec<Content>` canonically (architecture.md §3.1); a non-`Text` `Content` nested inside it that cannot be represented as tool-message text is a runtime degradation surfaced as `Error{kind: ParseInput}` (→ 64), the same text-only-slot rejection as §2.3 (architecture.md §3.1; §6 CR-5).
- `is_error` ← **no native field** (§6 CR-3). The flag cannot round-trip structurally; the adapter surfaces it **textually** by prefixing the content (e.g. `"[error] "`) so the model still sees the error signal.

This is the adapter owning its own projection (architecture.md §3.1). The fact that OpenAI *has* a `tool` role does not change the canonical truth: the core emits `Role::Tool`; this adapter spells it.

### 2.5 `tools[]`

`Tool::Custom{name, description, input_schema}` → `{type:"function", function:{name, description?, parameters}}`:

- `function.name` ← `name`.
- `function.description` ← `description` (omit when `None`).
- `function.parameters` ← `input_schema` (a JSON Schema `Value`; omitting it = empty params).

`strict` is not emitted unless present via `extra` (per-tool strict mode is out of the canonical set). A `Tool::Provider` (provider-typed enablement, architecture.md §3.1) is NOT projected here — encode rejects it with `ParseInput`/64 (§6, server-tools degradation).

### 2.6 `tool_choice` spellings

| `ToolChoice` | wire `tool_choice` |
|---|---|
| `Auto` | **omitted** (matches OpenAI's default; minimal body, architecture.md §3.1) |
| `Any` | `"required"` |
| `None` | `"none"` |
| `Tool{name}` | `{type:"function", function:{name}}` (the modern named form; **not** the deprecated bare `function_call`) |

### 2.7 Reasoning models — `max_completion_tokens` and dropped sampling

`req.reasoning` (the typed knob that also projects to `reasoning_effort`, providers.md §6) **IS the reasoning-model signal.** o-series/gpt-5 reasoning models — the exact models that accept `reasoning` — **reject** the deprecated `max_tokens` (they require `"max_completion_tokens"`) and **400 on a non-default `temperature`/`top_p`**. So `encode` reads that one explicit signal and adjusts two fields (never a model-name sniff — the no-vendor-match rule, architecture.md §3.1):

- **`max_tokens` → `max_completion_tokens` when `reasoning` is `Some`.** `req.max_tokens` emits under `"max_completion_tokens"` for a reasoning request, else the plain `"max_tokens"`. A row's `body_defaults.max_tokens` folds into the typed `req.max_tokens` at resolve (config §4.1), so the row default fills the *correct* key automatically — a reasoning request riding a row whose `body_defaults` sets `max_tokens` no longer 400s.
- **`temperature`/`top_p` omitted when `reasoning` is `Some`.** They stay on the canonical request, untouched, for every other protocol.

This is the **same rule the Anthropic encoder already applies** (anthropic-messages.md §2 / providers.md §6: extended thinking drops `temperature`/`top_p` and floors `max_tokens`) — mirroring it here removes an asymmetry that had no spec'd rationale, and dissolves the row-level workaround the live codex row needed (`unsupported_body_keys = ["max_output_tokens","temperature","top_p"]`, providers.md §9). A non-reasoning request against an o-series model driven WITHOUT `--reasoning` still reaches `max_completion_tokens` the row way: a raw `max_completion_tokens` in `body_defaults` (rides `extra`) plus `unsupported_body_keys = ["max_tokens"]`. No new flag in this impl (severability, architecture.md §4.6).

### 2.8 Streaming & usage

When `req.stream.unwrap_or(false)` is `true`, encode sets `"stream": true` **and** `"stream_options": {"include_usage": true}`. Without `include_usage`, OpenAI emits **zero** usage on a streamed response (§3.4). `include_obfuscation` is left default and ignored. When it is `false` (including an absent `None` stream), neither `stream` nor `stream_options` forces usage (a non-stream response carries `usage` in the body).

> **Cross-check note:** the paired `*_basic` cross-check fixtures (§5.1) deliberately run with `stream` such that **no `Usage` event is emitted on either side**, so the OpenAI/Anthropic basic equality is over the text skeleton only. This spec's own `openai_chat_basic` fixture (§5) does **not** set `include_usage`; a separate `openai_chat_usage` fixture exercises the usage path.

### 2.9 `Thinking` / `RedactedThinking` are dropped on re-send

Chat Completions assistant request messages carry **no** reasoning/thinking field; `Thinking.signature` **cannot round-trip** through this endpoint, and Anthropic's opaque `RedactedThinking.data` likewise has no slot. The adapter **drops** `Content::Thinking` and `Content::RedactedThinking` when projecting a request. This is a provider-inherent omission consistent with architecture.md §3.1 (signature is `None` for providers without the concept; redacted-thinking is never produced by a non-Anthropic adapter — the empty-set rule; adapters never fabricate). See §6 CR-2 — a change request is raised **only if** thinking replay through Chat Completions is ever required (it is not for v0.1; thinking replay rides the Responses API / Anthropic).

### 2.10 Headers set by `encode`

- `Content-Type: application/json` — set by `encode`.
- **No** `anthropic-version` / beta headers (those come from `ctx.beta_headers` only if the row defines any; the built-in OpenAI row defines none — architecture.md §4.2).
- `Authorization: Bearer {key}` — **set by `Auth::apply`, not encode** (architecture.md §4.5; OpenAI row `api_header = {name:"Authorization", scheme:"bearer"}`).

### 2.11 Worked request example

Canonical request (a tool-enabled, streaming turn with a prior tool result and an image):

```jsonc
// CanonicalRequest (canonical NDJSON-ish view)
{
  "model": "gpt-4o",                       // already alias-resolved
  "system": [{"type":"text","text":"You are concise."}],
  "messages": [
    {"role":"user","content":[
      {"type":"text","text":"What's in this image, and the weather in Paris?"},
      {"type":"image","source":{"kind":"base64","media_type":"image/png","data":"iVBORw0KG.."}}
    ]},
    {"role":"assistant","content":[
      {"type":"tool_use","id":"call_abc","name":"get_weather","input":{"location":"Paris"}}
    ]},
    {"role":"tool","content":[
      {"type":"tool_result","tool_use_id":"call_abc","content":[{"type":"text","text":"18C, clear"}],"is_error":false}
    ]}
  ],
  "tools": [
    {"name":"get_weather","description":"Current weather","input_schema":{"type":"object","properties":{"location":{"type":"string"}},"required":["location"]}}
  ],
  "tool_choice": {"type":"auto"},
  "temperature": 0.7,
  "stop": [],
  "stream": true
}
```

Emitted OpenAI `chat/completions` body:

```json
{
  "model": "gpt-4o",
  "messages": [
    {"role": "system", "content": "You are concise."},
    {"role": "user", "content": [
      {"type": "text", "text": "What's in this image, and the weather in Paris?"},
      {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KG.."}}
    ]},
    {"role": "assistant", "tool_calls": [
      {"id": "call_abc", "type": "function",
       "function": {"name": "get_weather", "arguments": "{\"location\":\"Paris\"}"}}
    ]},
    {"role": "tool", "tool_call_id": "call_abc", "content": "18C, clear"}
  ],
  "tools": [
    {"type": "function", "function": {
      "name": "get_weather", "description": "Current weather",
      "parameters": {"type": "object", "properties": {"location": {"type": "string"}}, "required": ["location"]}}}
  ],
  "temperature": 0.7,
  "stream": true,
  "stream_options": {"include_usage": true}
}
```

Note: `tool_choice` is **omitted** (canonical `Auto`); `stop` is **omitted** (empty); `top_p` is **omitted** (`None`); `max_tokens` is **omitted** (the OpenAI row requires none, so `req.max_tokens` is `None`); the assistant message carries `tool_calls` with **no** `content` key (nullable); `function.arguments` is a **JSON string**; the base64 image is a **data-URI inside `image_url.url`**; the tool result is a separate `role:"tool"` message keyed by `tool_call_id`; `stream_options` was added because `stream` is true.

---

## 3. RESPONSE mapping — `openai_chat` SSE stream → canonical `Vec<Event>`

`framing()` is `Framing::Sse`. The shared `SseDecoder` (the SSE-decoder spec (planned)) hands `decode` **one parsed `Frame`** at a time. For Chat Completions a successful-stream `Frame` is the payload of one `data:` line:

- A JSON object `{"object":"chat.completion.chunk", …}` (the normal case).
- The literal token `[DONE]` — **non-JSON**; the SSE layer special-cases it (parsing as JSON would throw) and hands `decode` a whole-payload `Frame` carrying the bytes `[DONE]`. `decode` maps it to `[]` (no events) and sets `state.terminated = true` — one of this protocol's two terminal markers (the other is a non-null `finish_reason` chunk, §3.6, CR-9).

There are **no `event:` lines** in this dialect (unlike Anthropic); the only discriminator is the JSON payload. A **non-2xx error body** reaches `decode` through a different decoder path (§4.0). `decode` is pure over `(frame, &mut DecodeState)`; all cross-frame state lives in `DecodeState`.

### 3.1 `DecodeState` for `openai_chat`

The state this protocol threads across frames (the slice of the shared `DecodeState` this impl owns; the SSE-decoder spec (planned) owns the type, the SSE buffer, and the `terminated: bool` flag):

```rust
struct OpenAiChatState {
    started: bool,                       // emitted MessageStart yet?
    text_open: Option<u32>,              // canonical index of the open text block, if any
    next_index: u32,                     // next canonical content index to assign
    tool_index: HashMap<u32, u32>,       // OpenAI tool_calls[].index -> canonical content index
    open: BTreeSet<u32>,                 // canonical indices with an open ContentStart (not yet stopped)
    refusal: String,                     // accumulated delta.refusal text (for Finish{Refusal})
}
```

`next_index`/`tool_index` keep OpenAI's `tool_calls[].index` namespace (0-based among tool calls) **separate** from the canonical content-block index: text block(s) occupy lower canonical indices, tool blocks get indices assigned in first-seen order (architecture.md §3.6 — "the adapter assigns one `index` space"). `open` is the source of truth for "which blocks must be closed at finish." The shared `terminated: bool` (architecture.md §3.5, CR-9) lives on `DecodeState`, not here — `decode` sets it when it consumes `[DONE]` **or a non-null `finish_reason` chunk** (§3.6).

> **Realization (single source of truth).** This struct is the *conceptual* slice; the impl threads it through the **shared `DecodeState`** (sse-decoder §5) and stores only what it cannot compute. `started`, `tool_index`, `refusal` are added fields; the shared `open: HashMap<u32, OpenBlock>` *is* the open-block set (its keys); and `next_index` (= `open.len()`) and `text_open` (the lone open `Text` block) are **computed from `open`**, never stored. `open.len()` equals the next index because content blocks are removed only at finish, all at once.

### 3.2 Chunk shape

```jsonc
{ "id":"chatcmpl-…", "object":"chat.completion.chunk", "created":<unix s>, "model":"gpt-4o-…",
  "choices":[ { "index":0, "delta":{…}, "finish_reason":null|"stop"|"length"|"tool_calls"|"content_filter"|"function_call", "logprobs":null } ],
  "usage": null | {…} }
```

`choices` is length 1 in scope (`n>1` is out of scope). `system_fingerprint`/`service_tier` are ignored (no canonical home). `delta` is the per-chunk increment.

### 3.3 Event-by-event mapping (per chunk, `choices[0]`)

**MessageStart (once).** On the first chunk seen (the role-only `{"role":"assistant","content":""}` delta carries it), if `state.started` is false: emit `MessageStart{ id: Some(chunk.id), model: Some(chunk.model), role: Role::Assistant }` (via `Event::message_start`, which stamps the constant `v`, architecture.md §3.2) and set `state.started = true`. `id`/`model` come from the **top-level** chunk fields. The role-only empty `content:""` does **not** open a text block (avoid an empty text block).

**Text.** On the first chunk with non-null `delta.content` (when `state.text_open` is `None`): **synthesize** `ContentStart{index: i, kind: ContentKind::Text {}}` where `i = state.next_index++`, set `state.text_open = Some(i)`, insert `i` into `state.open`. Then for that and every subsequent `delta.content` (non-null), emit `ContentDelta{index: i, delta: Delta::TextDelta(content)}`. This is how the **identity-precedes-content** invariant is met for OpenAI's contentless wire (architecture.md §3.2).

**Tool calls** (`delta.tool_calls[]`, the load-bearing case). Each element is `{index, id?, type?, function:{name?, arguments?}}`. OpenAI's `tool_calls[].index` (call it `t`):

- **First appearance of `t`** (the chunk where `id` + `function.name` are present — they appear **only once**, on the first chunk for that call): assign `c = state.next_index++`, record `state.tool_index[t] = c`, insert `c` into `state.open`, and **synthesize** `ContentStart{index: c, kind: ContentKind::ToolUse{ id, name }}`. The first chunk's `function.arguments` is typically `""` (an empty fragment — emit no `ContentDelta`; emitting an empty `JsonDelta` would be a no-op and is omitted for determinism under rechunking).
- **Subsequent fragments for `t`** (`id`/`type`/`name` null, only `function.arguments` present): look up `c = state.tool_index[t]` and emit `ContentDelta{index: c, delta: Delta::JsonDelta(arguments)}`. **Never parse mid-stream** — the fragments (`{"`, `location`, `":"Paris`, …) are valid JSON only when concatenated (architecture.md §3.2, §3.6). Assembly + parse-to-`Value` happens only when **folding** the event stream into `Content::ToolUse{id, name, input}` (the non-stream/fold path), never in `decode`.

**Refusal accumulation.** `delta.refusal` (non-null) is **not** a content block; append the fragment to `state.refusal`. It surfaces in the terminal `Finish{Refusal{…}}` (§3.5), never as a `ContentDelta`.

**Usage** (`usage` non-null on a chunk — see §3.4).

**Finish + ContentStop** (`finish_reason` non-null — the terminal content chunk for the choice, which carries an empty/near-empty `delta:{}`): first emit `ContentStop{index}` for **every** index in `state.open` (in ascending order), draining `state.open` — OpenAI sends no per-block stop, so it is synthesized here, guaranteeing the universal invariant "every `ContentDelta.index` has a following `ContentStop`" even on a finish that closes multiple open blocks. Then emit `Finish{reason}` per §3.5.

### 3.4 `usage` → `Event::Usage`

Usage is reported while streaming **only** if the request sent `stream_options.include_usage:true` (set by encode, §2.8). When enabled, exactly **one** extra chunk arrives **after** the finish chunk and **before** `[DONE]`, with `choices: []` (empty array) and a populated `usage`. All prior chunks carry `usage: null` (do **not** treat `null` as `0`). Map verbatim field names:

```rust
Event::Usage(Usage {
    input_tokens:       Some(usage.prompt_tokens),
    output_tokens:      Some(usage.completion_tokens),
    cache_read_tokens:  usage.prompt_tokens_details.and_then(|d| d.cached_tokens),  // Some iff present
    cache_write_tokens: None,                                                       // no OpenAI equivalent — never fabricate 0
})
```

`total_tokens` is derivable → dropped. Any absent field → `None` (architecture.md §3.2). Because the usage chunk is a **separate, later frame** than the finish chunk (§3.3), the emission order is `… ContentStop → Finish` (from the finish frame) then `Usage` (from the usage frame) — i.e. **`Finish` is emitted before `Usage`** (§3.6).

### 3.5 `finish_reason` + accumulated refusal → `FinishReason`

The terminal `Finish` reason is computed from `finish_reason` and `state.refusal` together, so **no accumulated refusal text is ever dropped** (the two OpenAI refusal mechanisms are decoupled):

| condition (at the finish frame) | canonical `Finish{reason}` |
|---|---|
| `state.refusal` non-empty (the model's structured `refusal` **output** field streamed via `delta.refusal`; this finishes with `finish_reason:"stop"`) | `FinishReason::Refusal{ category: "refusal".into(), explanation: Some(state.refusal) }` — **takes precedence** over the `finish_reason` mapping below |
| `state.refusal` empty **and** `finish_reason == "content_filter"` (a moderation stop; usually carries no `delta.refusal` text) | `FinishReason::Refusal{ category: "content_filter".into(), explanation: None }` |
| `state.refusal` empty **and** `finish_reason == "stop"` | `FinishReason::Stop` |
| `state.refusal` empty **and** `finish_reason == "length"` | `FinishReason::Length` |
| `state.refusal` empty **and** `finish_reason == "tool_calls"` | `FinishReason::ToolUse` |
| `state.refusal` empty **and** `finish_reason == "function_call"` (deprecated) | `FinishReason::ToolUse` |
| `state.refusal` empty **and** any other string `s` | `FinishReason::Other(s)` (never panics, architecture.md §9.5) |

**Precedence (when both fire):** a non-empty `state.refusal` wins regardless of `finish_reason`. If a refusal both streamed `delta.refusal` *and* finished with `content_filter`, the result is `Refusal{category:"refusal", explanation:Some(state.refusal)}` (the richer, content-bearing channel), not `content_filter`. Either way the result is a **`Finish{Refusal{..}}`** — HTTP 200, exit 0, **never** an `Error` (architecture.md §3.2, rule 6).

**Not produced by this adapter:** `FinishReason::StopSequence` (Chat Completions reports a stop-sequence hit as `"stop"`, not a distinct value — `StopSequence` is an Anthropic-only refinement, documented as excluded from the cross-check in §5.1) and `FinishReason::Pause` (Anthropic-only).

### 3.6 The terminators (and emission order)

`decode` **never emits `Event::End`.** The single `End` is the `sink.write(&Event::End)` the `run` loop appends once after the body iterator drains (architecture.md §4.4). `run` injects a premature-EOF `Error{Transport}` (exit 69) **only** when `!state.terminated` at body EOF (architecture.md §5.6, CR-9: a bare EOF with no decoded terminal marker is a premature drop; a cleanly-terminated stream is not).

**Two markers set `state.terminated = true` (bl-296d):**

1. **`data: [DONE]`** — OpenAI's sentinel; decodes to `[]` and flips `terminated`.
2. **A non-null `choices[0].finish_reason` chunk** — the same finish chunk that drains open blocks and emits `Finish` ALSO flips `terminated`.

**Why a `finish_reason` chunk is a terminal marker (the ruling).** OpenAI's own streams append `[DONE]` after the finish chunk, but a large share of the compat class this ONE row serves (Azure / OpenRouter / LiteLLM / vLLM / Mistral) **close the socket right after the `finish_reason` chunk with no `[DONE]`**. Were `[DONE]` the sole terminator, that clean completion would EOF with `!terminated` → a **spurious premature-EOF `Error{Transport}`/69 on a turn that actually finished** (the bl-296d second defect). Treating the non-null `finish_reason` chunk as terminal fixes this and matches the **field-on-chunk precedent of the sibling structureless dialects**: Google's non-null `finishReason` (providers.md §4.4) and Ollama's `{"done":true}` (§5.5) are each their protocol's terminator, and architecture.md §5.6 already names "a `finishReason`-bearing final chunk" in the terminal-marker set. It **loses nothing**: the finish→`[DONE]` window carries **no model output** — only the optional usage chunk, a metrics addendum whose absence is already tolerated (every `Usage` field is `Option`, §3.4). Truncation detection is preserved, because a truncated turn carries **no** `finish_reason` (neither marker fires → premature-EOF fires, correctly). The two markers are idempotent: an OpenAI stream flips `terminated` at the finish chunk and re-flips it (a no-op) at `[DONE]`. This same rule governs the non-stream fold (§3, decode_full), whose single folded chunk carries a non-null `finish_reason` — so `stream:false` now reports `terminated`, consistent with the Google/Ollama non-stream folds.

This is **consistent** with the Anthropic messages mapping (anthropic-messages.md) §3.8 (`message_stop` / the terminal `message_delta` are its markers) — **End ownership and the `terminated` discipline are the same on every protocol**, so feeding any through the shared `run` loop yields exactly one `End`.

Wire order is: `…content chunks… → finish chunk → (usage chunk, if include_usage) → [DONE]`. Mapped through the per-frame rules above, a fully-mapped basic stream (with usage) emits, in order:

```
MessageStart → ContentStart → ContentDelta* → ContentStop → Finish → Usage → End
```

`End` is appended by `run`, not by `decode`. (Without `include_usage`, the `Usage` event is absent and the order is `… → ContentStop → Finish → End`.)

### 3.7 Worked SSE → NDJSON trace (basic text, with usage)

Raw SSE in (each `data:` line is one `Frame`):

```
data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-2024-08-06","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}],"usage":null}

data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-2024-08-06","choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}],"usage":null}

data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-2024-08-06","choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":null}],"usage":null}

data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-2024-08-06","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":null}

data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-2024-08-06","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":2,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":0}}}

data: [DONE]
```

Canonical NDJSON out (one `Event` per line, per architecture.md §5.2 — the on-wire `serde` shape; `Event` is internally tagged on `"type"`, while `ContentKind` and `Delta` are **externally tagged**, so `ContentKind::Text {}` renders `"kind":{"text":{}}` and `Delta::TextDelta` renders `"delta":{"text_delta":"…"}`, matching architecture.md's §5.2 sample — CR-4, resolved in the architecture spec):

```
{"type":"message_start","id":"chatcmpl-9","model":"gpt-4o-2024-08-06","role":"assistant"}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}
{"type":"content_stop","index":0}
{"type":"finish","reason":"stop"}
{"type":"usage","input_tokens":12,"output_tokens":2,"cache_read_tokens":0,"cache_write_tokens":null}
{"type":"end"}
```

Frame-by-frame decode calls (each row = one `decode(frame, &mut state)` and the events it returns):

| frame | events returned | state mutation |
|---|---|---|
| role-only `content:""` | `MessageStart{…}` | `started=true` |
| `content:"Hel"` | `ContentStart{0,Text {}}`, `ContentDelta{0,TextDelta("Hel")}` | `text_open=Some(0)`, `next_index=1`, `open={0}` |
| `content:"lo"` | `ContentDelta{0,TextDelta("lo")}` | — |
| `finish_reason:"stop"` | `ContentStop{0}`, `Finish{Stop}` | `open={}`, **`terminated=true`** (finish is a terminal marker, §3.6) |
| usage chunk (`choices:[]`) | `Usage{input_tokens:12,output_tokens:2,cache_read_tokens:Some(0),cache_write_tokens:None}` | — |
| `[DONE]` | `[]` | `terminated` already set — idempotent no-op |
| *(body EOF)* | — | `terminated` is set, so `run` appends `End` with NO premature-EOF error (architecture.md §4.4, §5.6) |

Order is `ContentStop → Finish → Usage → End`: the finish frame emits `ContentStop` then `Finish` in one decode call **and flips `terminated`** (bl-296d); the **later** usage frame emits `Usage`; `[DONE]` emits nothing and re-flips `terminated` idempotently; `run` appends the one `End` at body EOF (and suppresses the premature-EOF injection because `terminated`). Had this compat server dropped the socket right after the finish frame with no usage chunk and no `[DONE]`, `terminated` would still be set → still no premature-EOF (the fix). This matches the §3.6 summary exactly.

(`cache_read_tokens` is `Some(0)` here because the wire reported `cached_tokens:0`; an **absent** `prompt_tokens_details`/`cached_tokens` would map to `None`, never to `0` — the distinction is load-bearing, architecture.md §3.2.)

### 3.8 Tool-call streaming trace (fragment example)

```
delta: {"role":"assistant","content":null}                          -> MessageStart
delta: {"tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"get_weather","arguments":""}}]}
                                                                     -> ContentStart{0, ToolUse{id:"call_x", name:"get_weather"}}   (tool_index{0->0})
delta: {"tool_calls":[{"index":0,"function":{"arguments":"{\""}}]}  -> ContentDelta{0, JsonDelta("{\"")}
delta: {"tool_calls":[{"index":0,"function":{"arguments":"location"}}]} -> ContentDelta{0, JsonDelta("location")}
delta: {"tool_calls":[{"index":0,"function":{"arguments":"\":\"Paris\"}"}}]} -> ContentDelta{0, JsonDelta("\":\"Paris\"}")}
finish_reason:"tool_calls"                                          -> ContentStop{0}, Finish{ToolUse}   (terminated=true, §3.6)
[DONE]                                                              -> []   (terminated already set; End appended by run at body EOF)
```

The concatenated `JsonDelta` fragments `{"` + `location` + `":"Paris"}` = `{"location":"Paris"}`, parsed to a `Value` only when folding to `Content::ToolUse{id:"call_x", name:"get_weather", input:{"location":"Paris"}}`.

---

## 4. ERROR mapping — HTTP status + body → `CanonicalError` + exit code

A Chat Completions failure takes **one of two shapes**: (a) a **handshake error** — the request never became a 2xx stream, so the whole response is a single JSON body with a non-2xx HTTP status (§4.0–§4.2, the common case); or (b) a **mid-stream in-band error** — a `data: {"error":…}` frame arrives ON an already-200 SSE stream (§4.3, the compat class). Per architecture.md §8, **provider-error parsing lives in `decode`** (pure, fixture-tested, no network); for (a) the **HTTP status is peeked separately** (`TransportResponse.status`, architecture.md §4.1) by `run` to drive the exit, while for (b) there is no governing status — `kind` derives from the body (CR-10, §4.3).

### 4.0 How a non-SSE error body reaches `decode` (the SSE-decoder contract)

`framing()` is `Framing::Sse`, but a non-2xx error body is a bare JSON object with no `data:` prefix and no SSE blank-line terminator — the SSE frame grammar would never yield a frame from it. The bridge is owned by the SSE-decoder spec (planned) and named here as a dependency so §4.1's `decode` parse is reachable:

> **Decoder contract (depended on by this spec and by the Anthropic messages mapping (anthropic-messages.md) §4.1):** when `TransportResponse.status` is **non-2xx**, the `run` loop / SSE decoder does **not** apply SSE framing; it hands `decode` the **whole response body as a single `Frame`** carrying that status as **`frame.status: Some(code)`** (sse-decoder §9). `decode` recognizes the whole-body error frame by `frame.status.is_some()` and parses the error envelope (§4.1). The carried status is the same status `run` peeks for the exit code — read by `decode`, never reconstructed from the body.

This is the **same decoder contract both protocol specs depend on** (the Anthropic messages mapping (anthropic-messages.md) §4.1). On a **2xx** stream the SSE path is used and error parsing never runs. `decode` emits `[Error(..)]` for the error frame; it does **not** emit `End` (the single `End` is appended by `run` at body EOF, §3.6).

### 4.1 Error body shape parsed in `decode`

```json
{ "error": { "message": "…", "type": "invalid_request_error", "param": "…|null", "code": "…|null" } }
```

`decode` defers this to the **one shared whole-body projection** (`json::http_error`, bl-5fe6) that every protocol's non-2xx path calls — there is no per-dialect error parsing:

```rust
Event::Error(CanonicalError {
    kind: ErrorKind::from_http_status(frame.status),  // the carried status — §4.2
    message: <best-effort summary, see below>,
    provider_detail: Some(<the WHOLE raw body, verbatim>),
})
```

It is emitted as **`Event::Error(..)`** — its own event, **never** folded into `Finish` (architecture.md §3.3). Field names are verbatim wire (`error.type`/`error.code`/`error.param` are strings|null — **not** the SDK exception class names). The exit code is computed from the **HTTP status**, not `error.type` (status drives kind/exit; the string lands in `provider_detail`). This is the **handshake error** (a non-2xx body); a non-2xx status is always present to drive `kind`, unlike a **mid-stream in-band error on a 2xx stream (§4.3)**, whose `kind` must instead derive from the BODY (CR-10 — there is no governing status to read). **The kind comes from the status *regardless of* whether the body parses:** a non-2xx with a non-JSON body (a proxy's HTML, an empty 5xx) still yields `Provider{status}`, not `Transport` — the carried status is authoritative and is never dropped on a parse failure.

**The RAW body is never discarded (bl-5fe6).** `provider_detail` carries the whole parsed body verbatim — NOT a presumed `{"error":…}` sub-object — so an envelope of any shape (OpenAI's `{"error":…}`, the ChatGPT/codex backend's flat `{"detail":…}`, a bare string) is diagnosable. A non-JSON body (proxy HTML, plain text) rides as a `Value::String` of its bytes; only a genuinely **empty** body degrades to `provider_detail: None`. `message` is a best-effort human summary pulled from a known field — nested `error.message`, a bare `error` string, or `detail` — else the body itself, so it is never empty when a body exists (text mode, which shows only `message`, stays diagnosable). The body is a RESPONSE; it carries no request creds, so there is no `Secret` to redact (architecture.md §6.4).

### 4.2 HTTP status → `ErrorKind` → exit code (per architecture.md §8)

This whole table **is** `ErrorKind::from_http_status(status)` (canonical model): `401|403 → Auth`, every other code → `Provider{status}`. Because `Provider{status}` already computes the exit (4xx→69, 5xx→70) and `retryable()` from the number, the status — once known — needs no second table; the `error.type` column below is the *typical* wire string (a diagnostic that rides `provider_detail`), **never** read for the kind. `Transport` is produced by the transport seam, not `decode`:

| HTTP status (typical `error.type`) | `ErrorKind` | exit | notes |
|---|---|---|---|
| `400` (`invalid_request_error`) | `Provider{status:400}` | **69** | a 400 **from the provider** is Provider-4xx (architecture.md §8). Distinct from adapter-side malformed-stdin rejection, which is `ParseInput`→64 (architecture.md §8). **The Anthropic messages mapping (anthropic-messages.md) §4.3 maps a provider 400 the same way** (Provider{400}→69) — the two adapters agree (§6, cross-spec note). |
| `401` (`invalid_api_key`) | `Auth` | **77** | |
| `403` (permission/region) | `Auth` | **77** | |
| `404` / `409` / `422` | `Provider{status}` | **69** | |
| `429` (`rate_limit_exceeded` / `insufficient_quota`) | `Provider{status:429}` | **69** | `retryable()` is **computed** (architecture.md §3.3): `true` for 429. `insufficient_quota` is effectively non-retryable but that is a downstream policy read of `provider_detail`, never a stored field. |
| `500` (`server_error`) | `Provider{status:500}` | **70** | `retryable()` → `true` (status ≥ 500). |
| `503` (overloaded) | `Provider{status:503}` | **70** | `retryable()` → `true`. |
| any other `4xx` | `Provider{status}` | **69** | |
| any other `5xx` | `Provider{status}` | **70** | |
| network / timeout / no HTTP response | `Transport` | **69** | `retryable()` → `true`; produced by the transport seam (architecture.md §8 `from_io`), not by `decode`. |

This is the OpenAI half of architecture.md §8's "4xx→69 (incl. 429) / 5xx→70 / 401-403→77", with 429's retryability living in the computed `retryable()`. Even under `--raw`, the status is peeked so a raw 4xx/5xx never exits 0 (architecture.md §5.4, §8).

### 4.3 Mid-stream in-band error on a 2xx stream (`data: {"error":…}`)

**Chat Completions DOES emit in-band SSE errors.** This corrects an earlier assumption (§6): the OpenAI *reference* backend generally reports failures at the handshake (§4.1), but the class this ONE dialect row serves by reuse — **Azure OpenAI, OpenRouter, LiteLLM, vLLM, Mistral** — routinely emits a `data: {"error":…}` frame **mid-stream, after the 200 handshake** (upstream rate-limit, an overloaded backend, a dropped upstream). The sibling Google spec documents the identical hazard (providers.md §4.8), and the Ollama (§5.9) and Anthropic (anthropic-messages.md §4.2) decoders each handle it. `decode` recognizes it after parse: a frame with `frame.status: None` whose parsed body has an `error` **object** is the in-band case; it is **surfaced as `Event::Error(..)`, never swallowed** (the bl-296d bug: the openai chat decoder alone read only `choices[0]`/`usage`, so an error frame produced zero events — the real error was discarded and the run mis-ended as premature-EOF/69, or silently exited 0 if a `[DONE]` followed).

**`kind` derives from the BODY (CR-10), not a status** — a 2xx stream has no governing HTTP status. The compat class is heterogeneous, so the projection reads whichever discriminator the body carries:

| body shape | `kind` | rationale |
|---|---|---|
| numeric `error.code` (e.g. `{"code":429}`, `{"code":503}`) | `ErrorKind::from_http_status(code)` | the OpenRouter/LiteLLM/proxy convention: a numeric `code` **is** an HTTP status — decoded through the one shared table (the Google §4.8 precedent), so `401\|403 → Auth`, else `Provider{code}`. |
| string `error.type` (or a string `error.code`) containing `rate_limit` / `quota` | `Provider{status:429}` → exit **69** | rate-limit-ish; `retryable()` → true. |
| string `type`/`code` containing `server` / `overload` / `unavailable` | `Provider{status:500}` → exit **70** | server/overloaded-ish; `retryable()` → true. |
| anything else (incl. absent `type`/`code`) | `Transport` → exit **69** | the honest read of a kindless / client-error body — retryable safe default. This mirrors the anthropic mid-stream table's `_ => Transport` fallback and Ollama's bare-string → Transport (§5.9). |

`message` ← `error.message`; **`provider_detail` ← the inner `error` object verbatim** (the diagnostic bytes, whatever shape — the sibling of §4.1's whole-body carry); **`retry_after_seconds` is inherently `None`** — a mid-stream 2xx error has no governing `Retry-After` handshake header (architecture.md §3.3, the field added in bl-135a).

**It does NOT set `state.terminated`.** An error frame is **not** a clean terminal marker (it is absent from architecture.md §5.6's marker set), so the arch's premature-EOF discipline is unchanged: if the socket then EOFs with no `[DONE]` and no `finish_reason` (§3.6), `run` **also** appends the premature-EOF `Error{Transport}` after the surfaced error — **last-error-wins** (architecture.md §8 CR-10 note), the same belt-and-suspenders "the stream did not cleanly complete" signal every sibling produces. The primary fix stands regardless: the real provider error is **surfaced with its own `kind`/`message`/`provider_detail`**, never discarded. When a `[DONE]` DOES follow the error frame (the "silent exit 0" case), that `[DONE]` sets `terminated`, no premature-EOF fires, and the surfaced `Event::Error` drives the exit — so the run no longer exits 0 on a truncated, errored turn.

---

## 5. Golden FIXTURES this protocol contributes

Per architecture.md §9.2, golden SSE captures live at `tests/fixtures/<name>.sse`, committed verbatim, decoded deterministically under adversarial rechunking (`OneByte`, `MidData`, `MidUtf8`, `MidJsonNumber`, `WholeFixture` — architecture.md §9.3); every strategy must yield an **identical** `Vec<Event>`. This protocol contributes (names align with architecture.md §9.2's `openai_chat_basic`, `openai_chat_tools`, `openai_error_4xx`/`401`):

| Fixture | Captures / asserts |
|---|---|
| `openai_chat_basic` | Basic text, **no `include_usage`** (so **no `Usage` event**). Decodes to `MessageStart → ContentStart{0,Text {}} → ContentDelta{0,TextDelta}* → ContentStop{0} → Finish{Stop}` (the `End` is appended by `run`). This is **this protocol's half of the cross-check** (§5.1). |
| `openai_chat_usage` | Basic text **with `stream_options.include_usage`** (the §3.7 trace). Asserts the usage chunk decodes to `Usage{input_tokens:Some, output_tokens:Some, cache_read_tokens:Some(0), cache_write_tokens:None}`, emitted **after** `Finish` and **before** the `run`-appended `End`. Pins the `Finish → Usage` ordering and the `cached_tokens:0`→`Some(0)` distinction. |
| `openai_chat_tools` | One tool call streamed as fragments (the §3.8 trace). Asserts: `ContentStart{ToolUse{id,name}}` synthesized on first sight (identity before content); `JsonDelta` fragments emitted raw, **never** parsed mid-stream; concatenation parses to the expected `input`; `Finish{ToolUse}`. |
| `openai_chat_refusal_filter` | `finish_reason:"content_filter"`, no `delta.refusal`, HTTP 200. Asserts `Finish{Refusal{category:"content_filter", explanation:None}}`, **exit 0**, and that **no `Event::Error`** is produced. |
| `openai_chat_refusal_field` | The model's structured `refusal` **output** field: `delta.refusal` fragments accumulate, `finish_reason:"stop"`, HTTP 200. Asserts `Finish{Refusal{category:"refusal", explanation:Some(<accumulated>)}}`, **exit 0**, no `Event::Error`, and that the accumulated refusal text is **not dropped** (§3.5 precedence). |
| `openai_error_401` | HTTP 401 error body (whole-body frame, §4.0). Asserts `Event::Error(CanonicalError{kind:Auth, message, provider_detail:Some})`, exit **77**, and **no `End` from `decode`** (the `End` is `run`-appended). |
| `openai_error_4xx` | HTTP 429 (`rate_limit_exceeded`) error body. Asserts `Provider{status:429}`, exit **69**, `retryable()==true`. A 400/`invalid_request_error` variant shares this family to cover the generic 4xx arm (Provider{400}→69, §4.2). |
| `openai_error_5xx` | HTTP 503/500 (`server_error`/overloaded). Asserts `Provider{status:5xx}`, exit **70**, `retryable()==true`. |
| `openai_chat_other_finish` | A deliberately-bogus `finish_reason` value. Asserts `FinishReason::Other(s)` (proves the no-panic-on-unknown contract executes, architecture.md §9.5). |
| `openai_chat_midstream_error_done` | A mid-stream `data: {"error":…}` frame (`rate_limit_error`) **followed by `[DONE]`** (§4.3, bl-296d). Asserts the error is SURFACED as `Event::Error{Provider{429}}` (exit 69, retryable, `retry_after_seconds:None`), not swallowed; the `[DONE]` sets `terminated`, so no premature-EOF. |
| `openai_chat_midstream_error_eof` | A mid-stream `data: {"error":…}` frame (`server_error`) **with EOF, no `[DONE]`** (§4.3). Asserts `Event::Error{Provider{500}}` (exit 70) is still surfaced, and that `terminated` stays **false** (an error is not a terminal marker) — so `run` would append the premature-EOF after it (last-error-wins). |
| `openai_chat_finish_no_done` | A `finish_reason:"stop"` chunk **with EOF, no `[DONE]`** (§3.6, bl-296d second defect). Asserts a clean `MessageStart → text → Finish{Stop} → End` with `terminated` **true** — proving `finish_reason` alone suppresses the spurious premature-EOF/69 a compat server would otherwise trigger. |

Universal invariants checked over **every** OpenAI fixture (architecture.md §9.2): decode + the `run`-appended terminator ends in exactly **one** `End`; `decode` itself emits **zero** `End` events; every `ContentDelta.index` has a preceding `ContentStart` and a following `ContentStop`; `Usage` fields are `Option`; and every fixture decodes **identically under whole-fixture vs one-byte rechunking** (arch §9.3). On a fixture that reaches a clean stop, `decode` sets `state.terminated` — at the **non-null `finish_reason` chunk** and/or `[DONE]` (§3.6, bl-296d); a mid-stream-error fixture sets it only if a `[DONE]` follows.

### 5.1 This protocol's half of the cross-check (basic-text pairing)

Per architecture.md §3.6 / §9.2, `openai_chat_basic.sse` and `anthropic_messages_basic.sse` represent the **same logical "basic text" response** — the assistant replying with the literal text **`Hello`** (chunked `"Hel"` + `"lo"`), finishing normally — and the property test asserts:

```
normalize(decode_all(openai)) == normalize(decode_all(anthropic))
```

To make the equality a single writable deterministic test, the paired `*_basic` fixtures are pinned so the reduced vectors are **byte-identical**:

- **Both `*_basic` fixtures OMIT usage.** `openai_chat_basic` sets no `include_usage` (so emits no `Usage` event); `anthropic_messages_basic`'s `message_start`/`message_delta` usage is dropped by `normalize`. Neither side's reduced vector contains a `Usage` event, so the load-bearing `cache_read_tokens:Some(0)`-vs-`None` distinction is never forced through `normalize` (the usage path is exercised separately by `openai_chat_usage` and the Anthropic usage fixtures, not by the cross-check). This is the **same convention the Anthropic messages mapping (anthropic-messages.md) §5.1 fixes** on its side.
- **`normalize` drops only provider-inherent identity:** `MessageStart.id`/`.model` (provider-specific identifiers — presence/shape is compared, not the literal strings) and any `Usage` event (per the bullet above). It drops nothing else.

The OpenAI half decodes, and reduces under `normalize`, to exactly:

```
MessageStart{ role: Assistant }            // id/model dropped: provider-inherent identity
ContentStart{ 0, Text {} }
ContentDelta{ 0, TextDelta("Hel") }
ContentDelta{ 0, TextDelta("lo") }
ContentStop{ 0 }
Finish{ Stop }
End                                        // appended once by run() after body EOF
```

This is **identical** to the reduced Anthropic vector (the Anthropic messages mapping (anthropic-messages.md) §5.1). The `(ContentStart, ContentDelta*, ContentStop)` triple is identical downstream whether synthesized here or native on Anthropic (architecture.md §3.2); the `MessageStart → text triple → Finish{Stop} → End` skeleton is byte-identical after `normalize`. That equality is the executable proof that the canonical model is one model, not two (architecture.md §3.6).

**Provider-inherent differences excluded from the equality (documented so no future pairing assumes equality):**

- **`Usage` presence/values** — OpenAI emits `Usage` only with `include_usage`; Anthropic emits it natively. Excluded by omitting usage on both `*_basic` sides. A usage cross-check is **not** writable as strict equality (`cache_read_tokens:Some(0)` vs `None` is a genuine value difference, architecture.md §3.2) and is not attempted.
- **`FinishReason::StopSequence` vs `Stop`** — a response that ended on a user stop sequence decodes to `StopSequence` on Anthropic but `Stop` on OpenAI (Chat Completions doesn't distinguish, §3.5). This is provider-inherent; the basic pairing uses normal completion (`stop`/`end_turn` → `Stop`), so it is not hit, and a stop-sequence pairing is **excluded** from the equality test (the Anthropic messages mapping (anthropic-messages.md) §5.1 documents the same exclusion).
- **A post-`MessageStart` `Usage` on Anthropic** — Anthropic may emit `Usage` immediately after `MessageStart`; OpenAI never does. Subsumed by the omit-usage convention above.

---

## 6. Edge cases & architecture change requests

**Decided edge cases (no change request needed — expressible today):**

- **Single-text vs multi-part content.** A single `Text` part SHOULD encode to a bare `"content":"…"` string; ≥2 parts or any non-text part use the array form. Decode dissolves the distinction (a bare string → `vec![Text]`, architecture.md §3.1). No branch survives downstream.
- **Empty role chunk.** `{"role":"assistant","content":""}` opens **no** text block; `MessageStart` is gated on `state.started` so it fires exactly once. Real text arriving later opens the block (§3.3).
- **`tool_calls[].index` namespace.** Kept distinct from canonical content index via `state.tool_index` (§3.1) — never assumed equal (text blocks may occupy lower canonical indices).
- **`usage:null`.** Never `0`; mapped to the absence of a `Usage` event for that chunk (§3.4).
- **`cached_tokens` absent vs `0`.** Absent `prompt_tokens_details`/`cached_tokens` → `cache_read_tokens:None`; present `0` → `cache_read_tokens:Some(0)`. Both faithful; `cache_write_tokens` is always `None` (§3.4).
- **Two refusal channels.** The structured `refusal` output field (streamed `delta.refusal`, finishes `"stop"`) and the `content_filter` moderation stop are **distinct** and both decoded; neither drops the other (§3.5).
- **Unknown `finish_reason`.** → `FinishReason::Other(s)`; never panics (§3.5, architecture.md §9.5).
- **`stop` empty.** Omitted, never sent as `[]`/`null` (§2.1).
- **`max_tokens` vs `max_completion_tokens`.** Resolved by the row/resolution layer; encode emits whichever key the resolved request carries (§2.7) — no `extra` override of a derived field, no new flag (§2.1.1, severability architecture.md §4.6).
- **End ownership & `terminated` (bl-296d).** `decode` never emits `End`; **TWO markers set `state.terminated = true`** — `[DONE]` **and** a non-null `finish_reason` chunk (§3.6); `run` appends the one `End` at body EOF and injects the premature-EOF `Error{Transport}` (exit 69) only when `!terminated` (architecture.md §5.6, CR-9). Adding `finish_reason` to the marker set fixes the compat-server-no-`[DONE]` false premature-EOF and aligns openai with the Google/Ollama field-on-chunk terminators (providers.md §4.4/§5.5); architecture.md §5.6 already lists "a `finishReason`-bearing final chunk." Consistent with the Anthropic messages mapping (anthropic-messages.md) §3.8 — no per-protocol terminator divergence.
- **Mid-stream in-band error on a 2xx stream (bl-296d — corrects an earlier misconception).** The prior text asserted "Chat Completions does not send in-band SSE `error` events on a 2xx stream." That is **false** for the Azure/OpenRouter/LiteLLM/vLLM/Mistral class this one row serves — they routinely emit `data: {"error":…}` mid-200-stream (the sibling Google spec, providers.md §4.8, documents the identical hazard). `decode` now handles it (**§4.3**): the error is surfaced as `Event::Error` with `kind` decoded from the BODY (CR-10 — no governing status), never swallowed. It does **not** set `terminated` (an error is not a clean terminal marker), so the §5.6 premature-EOF discipline is unchanged — a bare EOF after the error still injects the premature-EOF `Error{Transport}`, last-error-wins (architecture.md §8 CR-10 note), owned by `run`. The whole-body non-2xx handshake error (§4.1) is the distinct, status-driven case.
- **`RedactedThinking` is never produced by this adapter.** `ContentKind::RedactedThinking {}` / `Content::RedactedThinking{data}` exist canonically (architecture.md §3.1, §3.2) but are **Anthropic-only**; Chat Completions has no redacted-thinking wire block to decode into one, and the empty-set rule says a non-Anthropic adapter simply never emits it (decode side) and drops it on re-send (encode side, §2.9). A never-produced variant, by design — no change request.

**Architecture change requests (raised, scoped, NOT silently worked around):**

- **CR-1 — `Content::Image{Base64}` is structurally compressed but faithful.** Chat Completions has no structured `{media_type,data}` image source; base64 is embedded as a data-URI `data:{media_type};base64,{data}` inside `image_url.url`. This **round-trips** (decode re-parses the data URI back to `Base64{media_type,data}`), so **no architecture change is required** — recorded as a watch item only.

- **CR-2 — `Content::Thinking` has no request representation on Chat Completions.** `signature` cannot round-trip; the adapter **drops** `Thinking` (and the opaque `RedactedThinking`) on re-send (§2.9). Consistent with architecture.md §3.1 (signature `None` / never fabricated; redacted-thinking never produced by a non-Anthropic adapter). **No architecture change requested for v0.1** — thinking replay rides the Responses API / Anthropic. A change request is raised **only if** future requirements demand thinking replay *through Chat Completions*.

- **CR-3 — `Content::ToolResult.is_error` has no native Chat Completions field.** OpenAI tool messages carry no error flag. The adapter surfaces it **textually** (content prefix, §2.4) so the signal survives, but the structured boolean does **not** round-trip. **Change request to the architecture spec:** *if* a structured tool-result error channel must ever survive a Chat Completions round-trip, the architecture spec should bless the degradation rule ("textual surfacing, no canonical change") explicitly so this adapter is not silently lossy. Until then, a documented, intentional degradation.

- **Server tools (architecture.md CR-4, resolved there — this adapter's documented degradation).** The canonical `Tool::Provider{kind,name,config}` and `Content::ServerToolUse`/`ServerToolResult` (architecture.md §3.1) are Anthropic-carried opaque passthrough. This adapter does **NOT** project them: a `Tool::Provider` in `tools[]` **rejects at `encode`** with `ErrorKind::ParseInput` (exit 64, "provider-typed tools are not projected for this dialect") — fail fast, never a silent drop; a `Content::ServerTool*` block in a transcript hits the existing non-representable-content rejection the same way (`ParseInput`/64). Server-tool RESULT blocks are likewise never decoded/surfaced here (Chat Completions has no such wire block — the empty-set rule). `Tool::Custom` is unaffected (§2.5). Projecting provider-typed tools onto a dialect's native typed tools is future per-dialect work (the OpenAI **Responses** API has native typed tools — providers.md §9); no architecture change requested.

**Resolved in architecture.md (formerly CR-4, CR-5 — recorded here for provenance, no longer open):**

- **CR-4 (RESOLVED in architecture.md §3.2, §5.2) — `Delta`/`ContentKind` serde rendering.** The earlier draft flagged that an internally-tagged (`#[serde(tag="kind")]`) `Delta`/`ContentKind` could not serialize their newtype/unit variants to the documented `"delta":{"text_delta":"Hel"}` / `"kind":{"text":{}}` bytes. **The architecture spec now resolves this:** `ContentKind` and `Delta` drop internal tagging and use serde's default **external** tagging; `ContentKind`'s unit variants are **struct-like empty variants** (`Text {}`, `Thinking {}`, `RedactedThinking {}`) so external tagging yields `"kind":{"text":{}}`, and `Delta`'s newtype variants render `"delta":{"text_delta":"Hel"}`. `Event` keeps its `#[serde(tag="type")]` outer envelope, and `Event::Raw` is `#[serde(skip)]` (never serde-serialized — raw mode writes bytes verbatim, so `Raw` never appears as an NDJSON line). This spec's fixtures and the §3.7 trace are written against exactly these bytes; the architecture type, its §5.2 sample, and these fixtures now agree. No open request.

- **CR-5 (RESOLVED in architecture.md §3.1) — non-text `Content` in `system` / nested in `ToolResult.content`.** `req.system` and `ToolResult.content` stay permissive `Vec<Content>` canonically (the single source of truth holds any `Content`). **The architecture spec now blesses the runtime degradation:** an adapter whose target wire slot is **text-only** and that receives non-`Text` content **rejects at `encode`** with `ErrorKind::ParseInput` (exit 64) — a documented degradation in the affected encode direction, not a type change. This adapter applies it to the OpenAI `system`/`developer` slot (§2.3) and the `tool`-message content slot (§2.4). The Anthropic messages mapping (anthropic-messages.md) applies the identical rule to its own text-only slots. No open request.

---
