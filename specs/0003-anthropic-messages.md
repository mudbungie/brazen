# Spec 0003 — Canonical ⇄ Anthropic messages mapping

> **Status:** accepted · **Owner:** orionriver · **Derives from:** [0001 — Architecture & I/O Contract](0001-architecture.md)
> **Living document.** Edited like code. This spec is a **lossy projection** onto and back from the canonical model of spec 0001; it MUST NOT contradict 0001. Where the Anthropic wire cannot express a canonical fact (or vice-versa), this spec raises a **spec-0001 change request** (§6) rather than silently deviating.

---

## 1. Purpose & Scope

This spec defines the `AnthropicMessages` `Protocol` impl — the `protocol = "anthropic_messages"` registry entry of spec 0001 §4.2. It is **half of the v0.1 protocol set** (the other half is 0002, OpenAI chat/completions). It specifies, exactly and decisively:

- **REQUEST** (§2): how `encode(&CanonicalRequest, &ProviderCtx) -> WireRequest` projects every canonical field and every `Content` variant onto the `POST /v1/messages` JSON body + non-auth headers.
- **RESPONSE** (§3): how `decode(frame, &mut DecodeState) -> Vec<Event>` translates one parsed SSE frame of the Anthropic streaming response into ≥0 canonical `Event`s, and how `DecodeState` carries the cross-frame state.
- **ERRORS** (§4): how an HTTP status + error body maps to `CanonicalError{kind}` and the exit code of 0001 §8.
- **FIXTURES** (§5): the golden captures this protocol contributes to the test suite (0001 §9.2), including its half of the cross-protocol single-source-of-truth check.
- **EDGE CASES & CRs** (§6): the representational gaps and the change requests they imply.

### 1.1 Inherited invariants (from 0001 — restated so this spec is self-contained)

This impl is bound by every invariant in 0001 §3–§5. The load-bearing ones for the Anthropic mapping:

- **`Protocol` is PURE and object-safe.** `encode`/`decode`/`framing` touch no IO, no clock, no creds. Cross-frame state lives in the caller-owned `&mut DecodeState`, never on the impl, so `&AnthropicMessages` is shareable as `&'static dyn Protocol`.
- **The impl is vendor-blind.** It never sees `"anthropic"`; it reads only the capability projection `ProviderCtx { base_url, model (already alias-resolved), api_header, beta_headers, extra }`. The string `"anthropic"` is spent on the registry lookup before `encode` runs.
- **Auth is not Protocol.** `encode` sets **only** the body and non-auth headers (`content-type`, and `anthropic-version` from `ctx.beta_headers`). The `x-api-key` / `Authorization: Bearer` header is set by the `Auth` impl (0001 §4.5, §7), never here.
- **`content` is ALWAYS `Vec<Content>`.** A bare wire string decodes to `vec![Content::Text(..)]`; on encode, the array form is always safe.
- **`Thinking.signature` round-trips VERBATIM.** Never modified, never fabricated, never dropped.
- **Exactly ONE `Event::End` per response** (0001 §3.4).
- **Refusal is a `Finish{Refusal}`, never an `Error`** (0001 §3.2); it arrives HTTP 200, exit 0.
- **`Usage` fields are `Option`** — `None` is "unknown", never a fabricated `0` (0001 §3.2).
- **Tool-call arguments stream as `Delta::JsonDelta(String)` fragments**, parsed to `Value` only when folding to `Content::ToolUse` (0001 §3.6).
- **Identity precedes content:** `ContentStart{kind}` (carrying tool id/name) is emitted before any `ContentDelta` for that index. Anthropic gives this natively via `content_block_start` (§3.3).

### 1.2 `framing()`

```rust
fn framing(&self) -> Framing { Framing::Sse }
```

The Anthropic messages stream is Server-Sent Events. Each frame is `event: <name>\n` + `data: <JSON>\n\n`. The shared `SseDecoder` (0001 §9.3, spec 0006) yields one `Frame` per `data:` payload; `decode` parses that one frame's JSON and dispatches on its `type` field.

### 1.3 The provider row this impl is paired with

For reference (0001 §4.2 — this is **data**, not part of the impl):

```toml
[[provider]]
name = "anthropic"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "api_key"
api_header = { name = "x-api-key", scheme = "raw" }
beta_headers = [["anthropic-version", "2023-06-01"]]
default_max_tokens = 4096          # Anthropic REQUIRES max_tokens; folded at lowest precedence (flag > config > row)
```

`anthropic-version: 2023-06-01` reaches `encode` via `ctx.beta_headers`; it is **never hard-coded in this impl** (the impl is vendor-blind). `default_max_tokens = 4096` is folded during config resolution, so by `encode` time `req.max_tokens` is already `Some(..)` for any Anthropic row.

---

## 2. REQUEST mapping (canonical → `anthropic_messages` wire)

`encode` builds a `WireRequest` targeting `POST {ctx.base_url}/v1/messages` with body and headers below. Verified against the official reference (platform.claude.com/docs/en/api/messages, .../build-with-claude/extended-thinking, .../vision).

### 2.1 Non-auth headers (encode-set, via ctx)

| Header | Value | Source |
|---|---|---|
| `content-type` | `application/json` | constant (the only literal the impl hard-codes — it is protocol-inherent, not vendor identity) |
| `anthropic-version` | `2023-06-01` | `ctx.beta_headers` — **REQUIRED**; the request is rejected without it. NOT hard-coded. |
| `anthropic-beta` | `<id,id,…>` | comma-join of any further `ctx.beta_headers` entries; **omitted if none.** NOT required for a base request. |
| `x-api-key` / `Authorization` | — | **set by `Auth`, not `encode`.** (0001 §4.5) |

Concretely: `encode` copies every `(k, v)` in `ctx.beta_headers` onto the wire as-is (so `anthropic-version` and any beta land verbatim), then sets `content-type`. It sets no auth header.

### 2.2 Top-level body fields (canonical → wire)

| Wire field | Type | Canonical source | Rule |
|---|---|---|---|
| `model` | string | `ctx.model` | **REQUIRED.** Already alias-resolved; `encode` does NOT resolve aliases. |
| `max_tokens` | int | `req.max_tokens` | **REQUIRED by the API.** Already `Some(..)` by encode time (row `default_max_tokens` folded). A `None` reaching encode for an Anthropic row is a config-resolution bug — `encode` returns `Error{kind: Config}` (→ exit 78) rather than omit it. |
| `messages` | array | `req.messages` | REQUIRED. Projection in §2.3. |
| `system` | string \| `array<TextBlockParam>` | `req.system: Option<Vec<Content>>` | **TOP-LEVEL, not a message.** Omit if `None`. §2.4. |
| `temperature` | float | `req.temperature` | omit if `None`. |
| `top_p` | float | `req.top_p` | omit if `None`. |
| `stop_sequences` | `array<string>` | `req.stop` | **RENAME:** canonical `stop` → wire `stop_sequences`. Omit if empty. ⚠ OpenAI uses `stop`; easy to confuse. |
| `stream` | bool | `req.stream` | emit `true` when streaming. |
| `tools` | `array` | `req.tools` | omit if empty. §2.6. |
| `tool_choice` | object | `req.tool_choice` | §2.7. May be omitted when `Auto` (the default). |
| *(merged)* `extra` | various | `req.extra` (`#[serde(flatten)]`) | merged at the **top level** — carries `thinking`, `metadata`, `service_tier`, `top_k`, `cache_control`, `container`, `disable_parallel_tool_use`, etc. §2.8. |

`top_k`, `thinking`, `metadata`, `service_tier`, top-level `cache_control` are **not** first-class canonical fields — they ride `req.extra` and merge at the top level (§2.8). The canonical struct's typed fields take precedence over any same-named key in `extra` (the typed field is the single source of truth; `extra` is the long-tail valve).

### 2.3 `messages[]` projection (the load-bearing part)

Each wire entry is `{ "role": "user"|"assistant", "content": string | array<ContentBlockParam> }`. Canonical `content` is ALWAYS `Vec<Content>` → project to an array of content blocks. **Collapse to a bare string only** when the vec is exactly one `Text` and carries no `cache_control`; the array form is always wire-equivalent and is the safe default (it never loses `cache_control`).

**Role projection** (`Role` → wire `role`), **owned entirely by this adapter** — the core never branches on Anthropic's tool convention:

| Canonical `Role` | Wire | Notes |
|---|---|---|
| `User` | `"user"` | |
| `Assistant` | `"assistant"` | |
| `System` | *(not emitted inline)* | **Hoisted** to the top-level `system` field (§2.4). A `Message{role: System}` is NEVER written into `messages[]`. (The wire's inline `"system"` role exists only under the `mid-conversation-system` beta — out of scope; a need for it is a CR.) |
| `Tool` | `"user"` | **Adapter projection.** Anthropic has NO tool role. A `Message{role: Tool, content: [ToolResult..]}` emits `{"role":"user","content":[{"type":"tool_result",…}]}`. Adjacent `Tool` messages may be emitted as consecutive `"user"` messages (the API combines same-role) — merging is optional, not required. |

**Placement invariant (a 400 if violated):** `tool_use` blocks belong in **assistant** messages; `tool_result` blocks belong in **user** messages (which is exactly where the `Role::Tool` projection puts them). `thinking`/`redacted_thinking` blocks, when present in an assistant turn, MUST come **first** in that turn's content array, before any `tool_use`/`text`.

### 2.4 `system` handling

`req.system: Option<Vec<Content>>` is **hoisted out of `messages` to the top-level `system` field**:

- `None` → omit `system`.
- `Some(vec)` where every element is `Text` → emit `array<{type:"text","text":<s>}>`. Collapse to a bare string only if exactly one `Text` and no caching.
- `Some(vec)` containing a non-`Text` Content (Image/ToolUse/ToolResult/Thinking) → **UNREPRESENTABLE.** The wire `system` accepts text blocks only. `encode` returns `Error{kind: ParseInput}` (→ 64). This is a hard representational gap; see **CR-1** (§6).

### 2.5 `Content` variant → ContentBlockParam

| Canonical `Content` | Wire block |
|---|---|
| `Text(String)` | `{"type":"text","text":<s>}` |
| `Image{source: Base64{media_type,data}}` | `{"type":"image","source":{"type":"base64","media_type":<mt>,"data":<b64>}}` — `media_type` ∈ {`image/jpeg`,`image/png`,`image/gif`,`image/webp`} only |
| `Image{source: Url{url}}` | `{"type":"image","source":{"type":"url","url":<u>}}` |
| `ToolUse{id,name,input}` | `{"type":"tool_use","id":<id>,"name":<name>,"input":<input Value>}` — **assistant turn only** |
| `ToolResult{tool_use_id,content,is_error}` | `{"type":"tool_result","tool_use_id":<id>,"content":<string\|array<block>>,"is_error":<bool>}` — **inside a `"user"` message.** `content` is itself `Vec<Content>` → array of text/image blocks (or bare string if a single `Text`). `is_error` may be omitted when `false`. |
| `Thinking{text, signature: Some(sig)}` | `{"type":"thinking","thinking":<text>,"signature":<sig>}` — **`signature` passed VERBATIM**, never modified/omitted. |
| `Thinking{text, signature: None}` | A thinking block with no signature **cannot be replayed to Anthropic** (the API rejects a thinking block whose signature is absent on a multi-turn replay). On encode this is a representational gap; see **CR-2** (§6). v0.1 behavior: omit the block (do not fabricate a signature, do not send a signature-less thinking block that would 400). |

`redacted_thinking` (wire `{"type":"redacted_thinking","data":<opaque>}`) has **no canonical `Content` variant** — see **CR-3** (§6).

### 2.6 `tools` projection

Canonical `Tool{name, description: Option, input_schema: Value}` → flat wire object:

```json
{"name":<name>, "description":<desc?>, "input_schema":<JSON-Schema object>}
```

No `type` field for custom tools (the wire defaults to `"custom"`). `input_schema` must be a JSON Schema with `"type":"object"`. `description` is omitted when `None`. Built-in/server tools (bash, web_search, text_editor, …) use a `"type":"<versioned>"` discriminator and are **out of scope** for the canonical `Tool` (custom only); a server-tool need is a CR / `extra` passthrough.

### 2.7 `tool_choice` mapping

| Canonical `ToolChoice` | Wire |
|---|---|
| `Auto` | `{"type":"auto"}` — may be **omitted entirely** when there are no tools (the wire default) |
| `Any` | `{"type":"any"}` — (canonically the same intent as OpenAI's `"required"`) |
| `Tool{name}` | `{"type":"tool","name":<name>}` |
| `None` | `{"type":"none"}` |

The Anthropic-only `disable_parallel_tool_use: bool` modifier rides `req.extra` if needed (it merges onto the `tool_choice` object via the operator's `extra`), never a canonical field.

### 2.8 `extra` passthrough (Anthropic-specific knobs)

`req.extra` (`Map<String,Value>`, `#[serde(flatten)]`) is merged at the **top level** of the body. It carries everything Anthropic-specific that the canonical struct does not model:

- **`thinking`**: `{"type":"enabled","budget_tokens":N}` (N ≥ 1024 and N < `max_tokens`; valid on older models) · `{"type":"adaptive","display":"summarized"|"omitted"}` (Opus/Sonnet 4.6+; `budget_tokens` removed → 400 on 4.7/4.8/Fable 5) · `{"type":"disabled"}`. `display` defaults to `"summarized"` (older) / `"omitted"` (4.7/4.8/Fable 5).
- **`metadata`**: `{"user_id": <string>}`.
- **`service_tier`**: `"auto"|"standard_only"`.
- **`top_k`**: int.
- **`cache_control`**: `{"type":"ephemeral","ttl"?}` (top-level or per-block).
- **`container`**, **`disable_parallel_tool_use`**, etc.

`encode` performs a **shallow top-level merge**: it serializes the typed fields first, then folds in `extra` keys that are not already set by a typed field (typed field wins — single source of truth). This is the severability valve of 0001 §2 / §4.1: a new Anthropic knob needs **zero code**, only an `extra` key.

### 2.9 Worked REQUEST example

Canonical request (a tool round-trip continuation: a user asks for weather, the assistant already thought + called a tool, the tool result is fed back, and a system prompt is set):

```jsonc
// CanonicalRequest (canonical NDJSON-ish view)
{
  "model": "claude-opus-4-8",          // alias already resolved to wire id by resolution
  "system": [{"type":"text","text":"You are a terse weather bot."}],
  "messages": [
    {"role":"user","content":[{"type":"text","text":"Weather in SF?"}]},
    {"role":"assistant","content":[
      {"type":"thinking","text":"User wants current SF weather; call the tool.",
       "signature":"EqQBCgIYAhIM...VERBATIM..."},
      {"type":"tool_use","id":"toolu_01A","name":"get_weather",
       "input":{"location":"San Francisco, CA"}}
    ]},
    {"role":"tool","content":[
      {"type":"tool_result","tool_use_id":"toolu_01A",
       "content":[{"type":"text","text":"62F, foggy"}],"is_error":false}
    ]}
  ],
  "tools": [
    {"name":"get_weather","description":"Look up current weather",
     "input_schema":{"type":"object","properties":{"location":{"type":"string"}},
                     "required":["location"]}}
  ],
  "tool_choice": {"type":"auto"},
  "max_tokens": 1024,
  "temperature": 0.7,
  "stop": ["\n\nHuman:"],
  "stream": true,
  "thinking": {"type":"adaptive","display":"summarized"}   // from extra (#[serde(flatten)])
}
```

`encode` produces this wire body for `POST https://api.anthropic.com/v1/messages` (headers: `content-type: application/json`, `anthropic-version: 2023-06-01`; `x-api-key` added by Auth):

```json
{
  "model": "claude-opus-4-8",
  "max_tokens": 1024,
  "system": [{"type": "text", "text": "You are a terse weather bot."}],
  "messages": [
    {"role": "user", "content": [{"type": "text", "text": "Weather in SF?"}]},
    {"role": "assistant", "content": [
      {"type": "thinking",
       "thinking": "User wants current SF weather; call the tool.",
       "signature": "EqQBCgIYAhIM...VERBATIM..."},
      {"type": "tool_use", "id": "toolu_01A", "name": "get_weather",
       "input": {"location": "San Francisco, CA"}}
    ]},
    {"role": "user", "content": [
      {"type": "tool_result", "tool_use_id": "toolu_01A",
       "content": [{"type": "text", "text": "62F, foggy"}]}
    ]}
  ],
  "tools": [
    {"name": "get_weather", "description": "Look up current weather",
     "input_schema": {"type": "object",
                      "properties": {"location": {"type": "string"}},
                      "required": ["location"]}}
  ],
  "tool_choice": {"type": "auto"},
  "temperature": 0.7,
  "stop_sequences": ["\n\nHuman:"],
  "stream": true,
  "thinking": {"type": "adaptive", "display": "summarized"}
}
```

The four reframes to notice in this example:
1. `system` is hoisted to the **top level** (not a message); the `Role::System` content never appears in `messages[]`.
2. `Role::Tool` projected to a `"user"` message carrying a `tool_result` block.
3. canonical `stop` → wire `stop_sequences` (the rename), and `Thinking` placed **first** in the assistant turn with its `signature` byte-for-byte.
4. `thinking` (from `extra`) merged at the top level alongside the typed fields.

---

## 3. RESPONSE mapping (`anthropic_messages` stream → canonical `Vec<Event>`)

`decode(frame, &mut DecodeState)` parses ONE SSE frame's `data` JSON and returns ≥0 `Event`s. The SSE `event:` name and `data.type` are always consistent; **decode strictly against `data.type`** (the `event:` name is redundant). The impl is a pure state machine; all cross-frame state lives in `DecodeState`.

### 3.1 The wire flow

```
message_start
( content_block_start  content_block_delta*  content_block_stop )*   // one triple per block, keyed by index
message_delta+                                                       // 1+; final carries stop_reason + cumulative usage
message_stop
```

`ping` (`data: {"type":"ping"}`) may appear anywhere (keep-alive → zero events). A mid-stream `error` event may appear after the HTTP 200 (§4.2).

### 3.2 `DecodeState` (caller-owned cross-frame state)

The impl is pure over `(frame, &mut state)`; everything that must survive across frames lives here, NOT on the impl:

```rust
// fields this protocol requires of the shared DecodeState (spec 0006 owns the type)
struct DecodeState {
    open: HashMap<u32, OpenBlock>,   // index -> in-flight block, opened at content_block_start, removed at content_block_stop
    // (cumulative usage is re-emitted as revealed; no accumulation needed beyond
    //  what the wire already states cumulatively — see §3.6)
    // ...shared fields from 0006 (SSE buffering, etc.)
}

struct OpenBlock {
    kind: BlockKind,           // Text | ToolUse | Thinking  (drives folding)
    json: String,              // ToolUse only: concatenated input_json_delta fragments
    signature: Option<String>, // Thinking only: accumulated from signature_delta, attached at stop
}
```

`open` is keyed by the wire `index`, which is the single source of truth for "which block a delta routes to." A block is inserted at `content_block_start` and removed at `content_block_stop`.

### 3.3 How the ContentStart-before-deltas invariant is met

**Natively.** Anthropic's `content_block_start` carries the block's full identity *before any delta bytes* — for `tool_use`, the `id` and `name` are present at start (`input` is always `{}` at start; real args arrive as deltas). So `decode` emits `ContentStart{index, kind}` — with `kind: ContentKind::ToolUse{id, name}` already populated — the moment it sees `content_block_start`, and only then can any `ContentDelta{index, …}` for that index follow. No "have I seen the id yet?" branch is ever needed; the triple `(ContentStart, ContentDelta*, ContentStop)` is identical downstream to the OpenAI adapter's *synthesized* equivalent (0001 §3.6).

### 3.4 Event-by-event mapping

#### `message_start` → `MessageStart{id, model, role}` + (if present) `Usage`

```json
{"type":"message_start","message":{"id":"msg_…","role":"assistant","model":"claude-opus-4-8","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":25,"output_tokens":1}}}
```

- `message.id` → `MessageStart.id = Some(..)`; `message.model` → `MessageStart.model = Some(..)`; `message.role` is always `"assistant"` → `Role::Assistant`.
- `message.usage`, **if present**, → a `Usage` event (see §3.6 for field mapping). The usage object is **OPTIONAL** on `message_start` (in the thinking example it is absent entirely) — emit `Usage` only when present; never fabricate `0`.

Emits `[MessageStart, Usage?]` (the `Usage` only if `message.usage` exists).

#### `content_block_start` → `ContentStart{index, kind}`

The nested `content_block.type` selects `ContentKind`; `index` (u32) is the open-block key.

| wire `content_block.type` | `ContentKind` | DecodeState action |
|---|---|---|
| `text` (`{"type":"text","text":""}`) | `Text` | insert `OpenBlock{kind:Text}`. If the seed `text` is non-empty, also emit a `ContentDelta{index, TextDelta(seed)}` after the `ContentStart`. |
| `tool_use` (`{"type":"tool_use","id":"toolu_…","name":"get_weather","input":{}}`) | `ToolUse{id, name}` | insert `OpenBlock{kind:ToolUse, json:""}`. **Identity is native here.** `input` is always `{}`; ignore it (real args are deltas). |
| `thinking` (`{"type":"thinking","thinking":"","signature":""}`) | `Thinking` | insert `OpenBlock{kind:Thinking, signature:None}`. `thinking`/`signature` start empty. |
| `redacted_thinking` | — | no canonical variant — **CR-3** (§6). v0.1: see §6. |
| `server_tool_use` / `web_search_tool_result` / `fallback` | — | no canonical ContentKind — **CR-4** (§6); absent from the four core fixtures. |

Emits `[ContentStart{index, kind}]` (plus a `ContentDelta` for a non-empty text seed).

#### `content_block_delta` → `ContentDelta{index, delta}` (or DecodeState mutation)

`delta.type` selects the variant; `index` routes to the open block.

| wire `delta.type` | canonical | action |
|---|---|---|
| `text_delta` (`{"type":"text_delta","text":"Hello"}`) | `Delta::TextDelta(text)` | emit `ContentDelta{index, TextDelta}`. |
| `input_json_delta` (`{"type":"input_json_delta","partial_json":"{\"loc"}`) | `Delta::JsonDelta(partial_json)` | emit `ContentDelta{index, JsonDelta}` **AND** append `partial_json` to `open[index].json`. Fragments are valid **only concatenated**; the first is frequently `""`; a fragment may split mid-UTF-8 or mid-JSON-number. **NEVER parse mid-stream.** |
| `thinking_delta` (`{"type":"thinking_delta","thinking":"…"}`) | `Delta::ThinkingDelta(thinking)` | emit `ContentDelta{index, ThinkingDelta}`. |
| `signature_delta` (`{"type":"signature_delta","signature":"EqQB…"}`) | **not a `Delta` variant** | **do NOT emit a `ContentDelta`.** Store verbatim in `open[index].signature`. Arrives exactly once, immediately before `content_block_stop` of a thinking block. With `display:"omitted"` the thinking block gets ONLY this (zero `thinking_delta`). See **CR-5** (§6) if an event is ever needed. |

For `text_delta`/`input_json_delta`/`thinking_delta`: emits `[ContentDelta{…}]`. For `signature_delta`: emits `[]` (pure state mutation).

#### `content_block_stop` → `ContentStop{index}`

```json
{"type":"content_block_stop","index":0}
```

Closes the block at `index`: remove `open[index]`. (The folded `Content` value — `ToolUse{input: JSON.parse(json)}`, or `Thinking{text, signature}` with the accumulated signature attached — is materialized **only** when a consumer folds the event stream to `Content`; `decode` itself emits just the `ContentStop` marker. The fragments/signature in `DecodeState` are the source; the parse happens at fold time per 0001 §3.5/§3.6.) Emits `[ContentStop{index}]`.

#### `message_delta` → `Finish{reason}` (if terminal) + `Usage`

```json
{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}
```

- May appear **more than once**. `delta.stop_reason` is `null` on intermediate events; **emit `Finish` only when `stop_reason` is non-null** (the terminal one). Mapping in §3.5.
- `usage` here is **cumulative** and authoritative for the final `output_tokens` (and may restate input/cache fields) → a `Usage` event (§3.6).

Emits `[Usage?, Finish?]` — `Usage` whenever a `usage` object is present, `Finish` only when `stop_reason` is non-null. (A `Finish{reason}` arrives **before** `End` but they are distinct: `Finish` is *why* generation stopped; `End` is *the byte stream is over* — 0001 §5.6.)

#### `message_stop` → `End`

```json
{"type":"message_stop"}
```

→ **the ONE `Event::End`** (0001 §3.4). Native terminator. Exactly one per response. Emits `[End]`.

#### `ping` → nothing

`{"type":"ping"}` → emits `[]`. Pure keep-alive.

#### `error` (mid-stream, after HTTP 200) → `Error(CanonicalError)`

See §4.2 — a terminal stream error, not folded into `Finish`.

### 3.5 `stop_reason` → `FinishReason`

| wire `stop_reason` | `FinishReason` |
|---|---|
| `"end_turn"` | `Stop` |
| `"max_tokens"` | `Length` |
| `"stop_sequence"` | `StopSequence` (the matched string in top-level `stop_sequence` / `delta.stop_sequence` is **dropped** — `StopSequence` carries no payload; see CR-6) |
| `"tool_use"` | `ToolUse` |
| `"pause_turn"` | `Pause` (server-tool sampling loop hit its 10-iteration limit — only appears with server tools; resume by re-sending the assistant content as-is) |
| `"refusal"` | `Refusal{category, explanation}` — see §3.7 |
| `"model_context_window_exceeded"` | `Other("model_context_window_exceeded")` — no dedicated variant; do NOT conflate with `Length` (max_tokens). See **CR-7** (§6). |
| any unknown | `Other(s)` — never panics (0001 §9.5) |

### 3.6 `Usage` field mapping

Anthropic usage objects (on `message_start.usage` and the final `message_delta.usage`; NOT every `message_delta`) map field-by-field; every canonical field is `Option`, `None` when the wire omits it (never `0`):

| wire usage field | canonical `Usage` field |
|---|---|
| `input_tokens` | `input` (the **uncached** remainder; total prompt = `input` + `cache_write` + `cache_read`) |
| `output_tokens` | `output` (grows over the stream; the terminal `message_delta` is authoritative) |
| `cache_creation_input_tokens` | `cache_write` |
| `cache_read_input_tokens` | `cache_read` |
| `server_tool_use.web_search_requests` | *(no canonical field — ignored)* |

Usage is **cumulative**; emit a `Usage` event whenever a usage object is revealed. The initial `message_start` `output_tokens` is typically 1–3.

### 3.7 Refusal → `Finish{Refusal}` (HTTP 200, exit 0 — NEVER an Error)

A refusal arrives as **HTTP 200** with `stop_reason: "refusal"` and `content: []` (pre-output) — folded to `Finish{Refusal{category, explanation}}`, exit 0. Non-stream body shape:

```json
{"id":"msg_…","role":"assistant","content":[],"stop_reason":"refusal","stop_sequence":null,
 "stop_details":{"category":"cyber","explanation":"…"},"usage":{"input_tokens":100,"output_tokens":5}}
```

- `stop_details.category` (string, e.g. `"cyber"`, `"bio"`, may be `null`) → `Refusal.category` (`""` if `null`).
- `stop_details.explanation` (string|null) → `Refusal.explanation` (`Option`).
- `stop_details` is `null` for all other `stop_reason`s.

**Streaming refusal:** arrives as `message_delta` with `delta.stop_reason: "refusal"`. ⚠ The streaming doc samples show only `{stop_reason, stop_sequence}` on `message_delta.delta` and do **not** show `stop_details` riding the streamed `message_delta`. Whether `stop_details` is present in-stream is **UNVERIFIED against a live capture**; see **CR-8** (§6) and the `anthropic_messages_refusal` fixture (§5), which MUST be recorded from a real streamed refusal to settle this. If `stop_details` is absent in-stream, decode emits `Finish{Refusal{category:"", explanation:None}}`. **Never** an `Error`, always exit 0.

### 3.8 The one terminator → exactly one `End`

`message_stop` is the sole native terminator → exactly one `Event::End`. Note the asymmetry: a mid-stream `error` event is **terminal but is NOT followed by `message_stop`** (the wire sends no `message_stop` after a mid-stream error). To honor 0001's "exactly one `End`" invariant, **the adapter synthesizes `Event::End` after an `Error`** that terminates the stream. This synthesis is owned by the decode driver (the `run` loop already appends a final `End` — 0001 §4.4 emits `Event::End` after the body loop; the wire `message_stop`'s `End` and the driver's terminal `End` MUST NOT double-emit). **Resolution:** `decode` does NOT itself emit `End` for `message_stop` as a *second* End — it relies on the single `sink.write(&Event::End)` the `run` loop appends after the body iterator drains (0001 §4.4). On `message_stop`, decode emits `[]` and lets the body iterator reach EOF, after which `run` writes the one `End`. On a mid-stream `error`, decode emits `[Error(..)]`, the upstream closes, the body iterator reaches EOF, and `run` again appends the one `End`. **This yields exactly one `End` in both the normal and error-terminated cases without decode tracking terminator state.** (If a future SSE source did not close after `message_stop`, the shared decoder treats `message_stop` as upstream-complete and stops pulling — spec 0006 owns that detail.)

> **Note for 0006/implementation:** the cleanest realization of §3.8 is that the **`run` loop owns the single terminal `End`** (it already calls `sink.write(&Event::End)` once after the body drains, per 0001 §4.4), and `decode` never emits `End` itself. `message_stop` and mid-stream `error` both simply stop producing further events; the body iterator's EOF triggers the one `End`. This keeps "exactly one End" a structural property of `run`, not a thing decode must count.

### 3.9 Worked RESPONSE trace (SSE → NDJSON)

A "basic text" streamed response. **Input SSE** (`anthropic_messages_basic.sse`):

```
event: message_start
data: {"type":"message_start","message":{"id":"msg_01XYZ","role":"assistant","model":"claude-opus-4-8","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: ping
data: {"type":"ping"}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}
```

**Decoded `Vec<Event>`** (one decode call per frame; `[]` for `ping`):

```
MessageStart{id:Some("msg_01XYZ"), model:Some("claude-opus-4-8"), role:Assistant}
Usage{input:Some(12), output:Some(1), cache_read:None, cache_write:None}
ContentStart{index:0, kind:Text}
ContentDelta{index:0, delta:TextDelta("Hel")}
ContentDelta{index:0, delta:TextDelta("lo")}
ContentStop{index:0}
Usage{input:None, output:Some(2), cache_read:None, cache_write:None}
Finish{reason:Stop}
End                                  // appended once by run() after body EOF (§3.8)
```

**NDJSON on stdout** (0001 §5.2 — one Event per line, flushed, ending in `{"type":"end"}`):

```
{"type":"message_start","id":"msg_01XYZ","model":"claude-opus-4-8","role":"assistant"}
{"type":"usage","input":12,"output":1,"cache_read":null,"cache_write":null}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}
{"type":"content_stop","index":0}
{"type":"usage","input":null,"output":2,"cache_read":null,"cache_write":null}
{"type":"finish","reason":"stop"}
{"type":"end"}
```

This decode is **deterministic under adversarial rechunking** (0001 §9.3): feeding the same SSE bytes through `OneByte`, `MidData`, `MidUtf8`, `MidJsonNumber`, `WholeFixture` yields this exact `Vec<Event>`. `MidUtf8`/`MidJsonNumber` exercise the SseDecoder's partial-frame buffering (spec 0006); `decode` only ever sees a complete frame.

---

## 4. ERROR mapping (HTTP status + body → `CanonicalError` + exit code)

Per 0001 §8: every failure → `Event::Error(CanonicalError{kind, message, provider_detail})` AND a computed exit code. **The HTTP status is peeked separately from the body** (`TransportResponse.status`, 0001 §4.1) and drives the exit code; `decode` parses the body (pure, fixture-tested, no network) for `message`/`provider_detail`/`kind`.

### 4.1 The error body shape (parsed in decode)

Anthropic's error envelope is identical for HTTP errors and mid-stream SSE `error` events:

```json
{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},
 "request_id":"req_011CSHo…"}
```

- `error.message` → `CanonicalError.message`.
- the full `error` object → `CanonicalError.provider_detail` (verbatim `Value`).
- `error.type` informs the `kind` (table below) but is **NOT authoritative for the exit code** — the HTTP status is. `request_id` is present on HTTP responses, absent in SSE `error` events.

### 4.2 Mid-stream `error` event (after HTTP 200)

```
event: error
data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}
```

→ `Event::Error(CanonicalError{kind, message:"Overloaded", provider_detail})`. This is **terminal**: no `message_stop` follows. The status was already 200 when the stream opened, so the `kind` for a mid-stream error is derived from `error.type` (e.g. `overloaded_error` → `Provider{status:529}` by convention, since the original HTTP cannot re-signal it) and the exit follows §4.3. `decode` emits `[Error(..)]`; the single terminal `End` is appended by `run` at body EOF (§3.8). **Never folded into `Finish`** (0001 §3.2).

### 4.3 HTTP status / `error.type` → `ErrorKind` → exit code

| HTTP | `error.type` | `ErrorKind` | exit (0001 §8) |
|---|---|---|---|
| 400 | `invalid_request_error` | `ParseInput` (or `Config`/`Usage` per message) | 64 |
| 401 | `authentication_error` | `Auth` | 77 |
| 402 | `billing_error` | `Provider{status:402}` | 69 |
| 403 | `permission_error` | `Auth` | 77 |
| 404 | `not_found_error` | `Provider{status:404}` | 69 |
| 413 | `request_too_large` | `Provider{status:413}` | 69 |
| 429 | `rate_limit_error` | `Provider{status:429}` (retryable computed `true`) | 69 |
| 500 | `api_error` | `Provider{status:500}` | 70 |
| 504 | `timeout_error` | `Provider{status:504}` | 70 |
| 529 | `overloaded_error` | `Provider{status:529}` | 70 |

**Exit rule (from 0001 §8, restated):** 4xx (incl. 429) → **69**; 5xx (incl. 500/504/529) → **70**; 401/403 → **77**. `429` is `69` with `retryable()==true` computed (not a unique code — 0001 §8). `400` maps to `64` because malformed input is a usage/parse error, not a provider-availability error. The `error.type` string lands in `provider_detail`; the HTTP **status** (not the string) decides the exit.

`retryable()` is **computed** (`Transport`, or `Provider{status}` with `status==429 || status>=500`) — never stored (0001 §3.3). So `429`/`500`/`504`/`529` are retryable; `400`/`401`/`403`/`404`/`413` are not.

---

## 5. Golden FIXTURES this protocol contributes

Recorded from real streams, committed verbatim under `tests/fixtures/` (0001 §9.2). Each is decoded deterministically under adversarial rechunking incl. `MidUtf8` / `MidJsonNumber` (0001 §9.3). This protocol contributes:

| Fixture | Captures | Decodes to (shape) |
|---|---|---|
| `anthropic_messages_basic` | basic text stream | `MessageStart, Usage, ContentStart{0,Text}, ContentDelta{0,TextDelta}*, ContentStop{0}, Usage, Finish{Stop}, End` (the §3.9 trace) |
| `anthropic_messages_tools` | text block (idx 0) then `tool_use` (idx 1) with `input_json_delta` fragments (first is `""`) | `ContentStart{1,ToolUse{id,name}}` emitted **before** any `JsonDelta` (native identity-first); `Finish{ToolUse}` |
| `anthropic_messages_thinking_tools` | thinking block w/ `signature` (carries a `signature_delta`), then tool/text | `ContentStart{Thinking}` → `ThinkingDelta*` → (signature accumulated, NOT a ContentDelta) → `ContentStop`; signature round-trips verbatim into `Content::Thinking.signature` at fold |
| `anthropic_messages_refusal` | **HTTP 200**, `stop_reason:"refusal"` + `stop_details`, empty content | `Finish{Refusal{category,explanation}}`, **exit 0** — NOT an `Error`. (Recorded from a live **streamed** refusal to settle CR-8 / §3.7.) |
| `anthropic_messages_pause` | server-tool loop hits 10-iteration limit → `stop_reason:"pause_turn"` | `Finish{Pause}` (carries a `server_tool_use` block with no result — see CR-4) |
| `anthropic_error_overloaded` | **HTTP 529** `event: error` / `overloaded_error` | `Error(CanonicalError{Provider{529}, "Overloaded", provider_detail})`, **exit 70** |

(The OpenAI side contributes `openai_chat_basic`, `openai_chat_tools`, `openai_error_4xx`/`401` per spec 0002 / 0001 §9.2.)

### 5.1 This protocol's half of the cross-check (the single-source-of-truth proof)

0001 §3.6 / §9.2 require: `anthropic_messages_basic.sse` and `openai_chat_basic.sse` represent the **same logical response** and a property test asserts `normalize(decode_all(anthropic)) == normalize(decode_all(openai))`, where `normalize` drops only provider-inherent identity and `Option` fields a provider genuinely omits.

**Anthropic's half** is fixed precisely so the equality is writable: `anthropic_messages_basic` is the assistant replying with the literal text **`Hello`** (chunked `"Hel"` + `"lo"`), `stop_reason:"end_turn"`, and a usage of `input_tokens:12, output_tokens:2`. Its decoded `Vec<Event>` (the §3.9 trace) reduces under `normalize` to:

```
MessageStart{role:Assistant}              // id/model dropped: provider-inherent identity
ContentStart{0, Text}
ContentDelta{0, TextDelta("Hel")}
ContentDelta{0, TextDelta("lo")}
ContentStop{0}
Finish{Stop}
End
```

`normalize` drops, for the cross-check: `MessageStart.id`/`.model` (provider-specific identifiers); intermediate `Usage` events and `Usage` field values (OpenAI only reports usage with `stream_options:{include_usage:true}`, and cache fields differ) — i.e. the **presence/values of `Usage`** are normalized out, only the `Usage` *None-vs-Some of input/output* equivalence the providers share is checked, per 0001's "Option fields a provider genuinely omits." The text content, the `(ContentStart, ContentDelta*, ContentStop)` triple structure, `Finish{Stop}`, and the single `End` are **identical** to OpenAI's decoded basic fixture. The matching `openai_chat_basic` (spec 0002) MUST encode the same logical "Hello" reply so this equality holds — that pairing is what proves the canonical model is one model, not two.

Plus the universal invariants over **every** Anthropic fixture (0001 §9.2): decode ends in exactly one `End`; every `ContentDelta.index` has a preceding `ContentStart` and a following `ContentStop`; `Usage` fields are `Option`.

---

## 6. Edge cases & spec-0001 change requests

Every item below is a place the Anthropic wire and the canonical model of 0001 do not align. Each is either resolved within this spec (no CR) or raised as a **change request to 0001** (the canonical type genuinely cannot express the fact). Per the derivation rule, none is silently deviated.

### Resolved here (no canonical change)

- **`stop` → `stop_sequences` rename** (§2.2): a pure field-name projection; canonical `stop` is unchanged.
- **`Role::Tool` → `"user"` + `tool_result`** (§2.3): the adapter owns this projection; the core never branches on it (0001 §3.1). No change.
- **`system` hoisting** (§2.4): `Role::System` content / `req.system` → top-level `system`. The canonical model already separates `req.system` from `messages`; this is just where the wire puts it. No change.
- **`tool_choice` spellings** (§2.7): the four canonical intents map cleanly to the four wire shapes. No change.
- **Single-Text collapse to bare string** (§2.3/§2.4): wire-equivalent; always safe to emit the array. No change.
- **`extra` top-level merge** (§2.8): the severability valve of 0001 §2/§4.1, exactly as intended. No change.
- **`signature_delta` is not a `Delta`** (§3.4): accumulated in `DecodeState`, attached at fold — fully expressible. No change *unless* §CR-5.
- **One `End` despite no `message_stop` after a mid-stream error** (§3.8): resolved by `run` owning the single terminal `End`; decode never emits `End`. No change.

### Change requests to spec 0001 (genuine representational gaps)

- **CR-1 — non-Text content in `system`.** The wire `system` accepts text blocks only; canonical `req.system: Option<Vec<Content>>` permits any `Content`. v0.1 errors (`ParseInput`/64) on non-Text system content. *Requested change:* either constrain `req.system` to `Vec<TextBlock>` at the type level (single source of truth — make the gap unrepresentable), or document the runtime rejection as canonical. **Decision needed in 0001.** (Low urgency — non-Text system is rare.)

- **CR-2 — `Thinking{signature: None}` is not replayable to Anthropic.** Anthropic 400s on a thinking block whose signature is absent on multi-turn replay. The canonical `Thinking.signature: Option<String>` permits `None` (for providers without signatures). v0.1 drops a signature-less thinking block on encode rather than 400. *Requested clarification:* confirm 0001's intent that a `None`-signature `Thinking` is **not** round-trippable to Anthropic and dropping is correct (the empty-set/`None` semantics of 0001 §3.1 already imply this — likely **no type change**, just a documented encode rule). Recorded as a CR for visibility.

- **CR-3 — `redacted_thinking` has no canonical `Content` variant.** Wire `{"type":"redacted_thinking","data":<opaque>}` round-trips verbatim like a signature, and the API 400s if thinking/redacted_thinking blocks are modified, reordered, or filtered. Canonical `Content` has only `Thinking{text, signature}`. *Requested change:* add `Content::RedactedThinking { data: String }` (opaque, round-tripped verbatim). v0.1 interim: **map `redacted_thinking` → `ContentKind::Thinking`** carrying the opaque blob in the `signature` slot (so it survives a round-trip) — but this is a **lossy hack** (it conflates two wire blocks); the clean fix is the new variant. **This is the strongest CR — the core fixtures avoid it, but real extended-thinking streams hit it.**

- **CR-4 — server-tool blocks have no canonical `ContentKind`.** `server_tool_use`, `web_search_tool_result`, and `fallback` (a start/stop pair with zero deltas) appear in real streams (and in `anthropic_messages_pause`, which carries a `server_tool_use` block). `message_delta` may also carry `usage.server_tool_use.web_search_requests`, which has no canonical `Usage` field. *Requested change:* decide whether server tools get canonical content kinds or stay `Raw`/`extra`. The four core fixtures (basic/tools/thinking/refusal) avoid them; `pause` and any web-search fixture hit them. **Block any web-search fixture on this CR.**

- **CR-5 — `signature_delta` as a stream event.** Canonical `Delta` has no `SignatureDelta`. v0.1 accumulates it in `DecodeState` and never surfaces it as a streamed event (it appears only on the folded `Content::Thinking.signature`). If a consumer needs the signature *mid-stream* (before fold), 0001 would need a `Delta::SignatureDelta(String)` (or a dedicated event). v0.1 does **not** need this — flagged only if a downstream use emerges. **Low urgency.**

- **CR-6 — `StopSequence` drops the matched string.** When `stop_reason=="stop_sequence"`, the wire reports the matched sequence in top-level `stop_sequence` / `delta.stop_sequence`. Canonical `FinishReason::StopSequence` carries **no payload**, so the matched string is dropped. *Requested change (if the matched sequence must be preserved):* `StopSequence { matched: String }`. v0.1 drops it (most callers set the stop sequence themselves and don't need it echoed). **Low urgency.**

- **CR-7 — `model_context_window_exceeded` has no dedicated `FinishReason`.** Mapped to `Other("model_context_window_exceeded")`. It is semantically distinct from `Length` (max_tokens): the former means prompt+output hit the context window, the latter means the requested output cap was hit. `Other(String)` correctly avoids conflation and never panics (0001 §9.5), so **no change is strictly required** — raised only if callers need to branch on it as a first-class reason.

- **CR-8 — streamed refusal `stop_details` is unverified.** §3.7: the streaming doc shows only `{stop_reason, stop_sequence}` on `message_delta.delta`; whether `stop_details{category, explanation}` rides the streamed `message_delta` is **not documented**. *Action (not a type change):* the `anthropic_messages_refusal` fixture (§5) MUST be recorded from a **live streamed refusal** to settle whether `category`/`explanation` are available in-stream. If absent, decode emits `Finish{Refusal{category:"", explanation:None}}` (still exit 0, still never an `Error`). **This blocks finalizing the refusal fixture, not the canonical model.**

---

## 7. Summary of decisions (this spec is decisive)

- **Framing:** `Sse`. Decode against `data.type`, not the `event:` name.
- **Request:** `system` hoisted top-level; `Role::Tool` → `"user"`+`tool_result`; `stop` → `stop_sequences`; `Thinking` first in assistant turn with `signature` verbatim; `max_tokens` required (config-resolution guarantees `Some`); `extra` merged top-level (typed fields win); auth headers set by `Auth`, not encode; `anthropic-version` from `ctx.beta_headers`.
- **Response:** native `content_block_start` satisfies identity-before-content; `input_json_delta` → `JsonDelta`, never parsed mid-stream; `signature_delta` accumulated in `DecodeState`, not a `Delta`; `Usage` cumulative and `Option`; `message_delta` emits `Finish` only when `stop_reason` non-null.
- **Refusal:** `Finish{Refusal}`, HTTP 200, exit 0 — never an `Error`.
- **Terminator:** `message_stop` and mid-stream `error` both stop producing events; `run` appends the single `End` at body EOF — exactly one `End` in all cases.
- **Errors:** HTTP status drives the exit (4xx→69 incl. 429, 5xx→70, 401/403→77, 400→64); `error.type` informs `kind` and lands in `provider_detail`; `retryable()` computed.
- **CRs raised:** CR-3 (redacted_thinking — strongest), CR-4 (server tools), CR-1 (non-Text system), plus CR-2/5/6/7/8 documented for visibility. None silently deviated.

CITATIONS: https://platform.claude.com/docs/en/api/messages · https://platform.claude.com/docs/en/build-with-claude/streaming · https://platform.claude.com/docs/en/api/errors · https://platform.claude.com/docs/en/build-with-claude/extended-thinking · https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons · https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback · https://platform.claude.com/docs/en/build-with-claude/vision
