# Canonical ⇄ Anthropic messages mapping

> **Living document.** Edited like code. This spec is a **lossy projection** onto and back from the canonical model of the architecture spec; it MUST NOT contradict it. Where the Anthropic wire cannot express a canonical fact (or vice-versa), this spec raises a **change request to architecture.md** (§6) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md)

---

## 1. Purpose & Scope

This spec defines the `AnthropicMessages` `Protocol` impl — the `protocol = "anthropic_messages"` registry entry of architecture.md §4.2. It is **half of the v0.1 protocol set** (the other half is the OpenAI chat mapping (openai-chat-mapping.md), OpenAI chat/completions). It specifies, exactly and decisively:

- **REQUEST** (§2): how `encode(&CanonicalRequest, &ProviderCtx) -> WireRequest` projects every canonical field and every `Content` variant onto the `POST /v1/messages` JSON body + non-auth headers.
- **RESPONSE** (§3): how `decode(frame, &mut DecodeState) -> Vec<Event>` translates one parsed SSE frame of the Anthropic streaming response into ≥0 canonical `Event`s, and how `DecodeState` carries the cross-frame state.
- **ERRORS** (§4): how an HTTP status + error body maps to `CanonicalError{kind}` and the exit code of architecture.md §8.
- **FIXTURES** (§5): the golden captures this protocol contributes to the test suite (architecture.md §9.2), including its half of the cross-protocol single-source-of-truth check.
- **EDGE CASES & CRs** (§6): the representational gaps and the change requests they imply.

### 1.1 Inherited invariants (from architecture.md — restated so this spec is self-contained)

This impl is bound by every invariant in architecture.md §3–§5. The load-bearing ones for the Anthropic mapping:

- **`Protocol` is PURE and object-safe.** `encode`/`decode`/`framing` touch no IO, no clock, no creds. Cross-frame state lives in the caller-owned `&mut DecodeState`, never on the impl, so `&AnthropicMessages` is shareable as `&'static dyn Protocol`.
- **The impl is vendor-blind.** It never sees `"anthropic"`; it reads only the capability projection `ProviderCtx { base_url, model (already alias-resolved), beta_headers }`. The string `"anthropic"` is spent on the registry lookup before `encode` runs.
- **Auth is not Protocol.** `encode` sets **only** the body and non-auth headers (`content-type`, and `anthropic-version` from `ctx.beta_headers`). The `x-api-key` / `Authorization: Bearer` header is set by the `Auth` impl (architecture.md §4.5, §7), never here.
- **`content` is ALWAYS `Vec<Content>`.** A bare wire string decodes to `vec![Content::Text(..)]`; on encode, the array form is always safe.
- **`Thinking.signature` round-trips VERBATIM.** Never modified, never fabricated, never dropped.
- **`Content::RedactedThinking{data}` round-trips VERBATIM.** The opaque `data` blob is carried byte-for-byte; the API 400s if `thinking`/`redacted_thinking` blocks are altered, reordered, or dropped on multi-turn replay (architecture.md §3.1).
- **Exactly ONE `Event::End` per response** (architecture.md §3.4). `decode` **never emits `End`** — the single terminator is the `sink.write(&Event::End)` the `run` loop appends once after the body iterator drains (architecture.md §4.4); `message_stop` decodes to `[]` and sets `DecodeState.terminated = true` (§3.8). **End ownership is identical in the sibling OpenAI chat mapping (openai-chat-mapping.md) §3.6** — `[DONE]` there also decodes to `[]` and sets `terminated`.
- **Refusal is a `Finish{Refusal}`, never an `Error`** (architecture.md §3.2); it arrives HTTP 200, exit 0.
- **`Usage` fields are `Option`** — `None` is "unknown", never a fabricated `0` (architecture.md §3.2).
- **Tool-call arguments stream as `Delta::JsonDelta(String)` fragments**, parsed to `Value` only when folding to `Content::ToolUse` (architecture.md §3.6).
- **Identity precedes content:** `ContentStart{kind}` (carrying tool id/name) is emitted before any `ContentDelta` for that index. Anthropic gives this natively via `content_block_start` (§3.3).

### 1.2 `framing()`

```rust
fn framing(&self) -> Framing { Framing::Sse }
```

The Anthropic messages stream is Server-Sent Events. Each frame is `event: <name>\n` + `data: <JSON>\n\n`. The shared `SseDecoder` (architecture.md §9.3, the SSE-decoder spec) yields one `Frame` per `data:` payload; `decode` parses that one frame's JSON and dispatches on its `type` field. A **non-2xx error body** reaches `decode` through a separate SSE-decoder-spec path (§4.0).

### 1.3 The provider row this impl is paired with

For reference (architecture.md §4.2 — this is **data**, not part of the impl):

```toml
[[provider]]
name = "anthropic"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "api_key"
api_header = { name = "x-api-key", scheme = "raw" }
beta_headers = [["anthropic-version", "2023-06-01"]]
body_defaults = { max_tokens = 4096 }   # Anthropic REQUIRES max_tokens; folded at lowest precedence (flag > config > row), config §4.1
```

`anthropic-version: 2023-06-01` reaches `encode` via `ctx.beta_headers`; it is **never hard-coded in this impl** (the impl is vendor-blind). `body_defaults = { max_tokens = 4096 }` is folded into `cfg.max_tokens` during config resolution (config §4.1), so by `encode` time `req.max_tokens` is already `Some(..)` for any Anthropic row.

---

## 2. REQUEST mapping (canonical → `anthropic_messages` wire)

`encode` builds a `WireRequest` targeting `POST {ctx.base_url}/v1/messages` with body and headers below. Verified against the official reference (platform.claude.com/docs/en/api/messages, .../build-with-claude/streaming, .../vision).

### 2.1 Non-auth headers (encode-set, via ctx)

| Header | Value | Source |
|---|---|---|
| `content-type` | `application/json` | constant (the only literal the impl hard-codes — it is protocol-inherent, not vendor identity) |
| `anthropic-version` | `2023-06-01` | `ctx.beta_headers` — **REQUIRED**; the request is rejected without it. NOT hard-coded. |
| `anthropic-beta` | `<id,id,…>` | comma-join of any further `ctx.beta_headers` entries; **omitted if none.** NOT required for a base request. |
| `x-api-key` / `Authorization` | — | **set by `Auth`, not `encode`.** (architecture.md §4.5) |

Concretely: `encode` copies every `(k, v)` in `ctx.beta_headers` onto the wire as-is (so `anthropic-version` and any beta land verbatim), then sets `content-type`. It sets no auth header.

### 2.1.1 `extra` precedence (single source of truth)

`req.extra` (`Map<String,Value>`, `#[serde(flatten)]`) merges at the **top level** of the body (§2.8). `encode` serializes the typed canonical fields **first**, then folds in `extra` keys that are **not already set by a typed field** — **the typed field wins** (it is the single source of truth; `extra` is the long-tail valve, architecture.md §3.1). `extra` MUST NOT override a typed-field-derived value. This is the **same precedence rule as the sibling OpenAI chat mapping (openai-chat-mapping.md) §2.1.1** — the two protocol adapters give `extra` identical precedence.

### 2.2 Top-level body fields (canonical → wire)

| Wire field | Type | Canonical source | Rule |
|---|---|---|---|
| `model` | string | `ctx.model` | **REQUIRED.** Already alias-resolved; `encode` does NOT resolve aliases. |
| `max_tokens` | int | `req.max_tokens` | **REQUIRED by the API.** Already `Some(..)` by encode time (row `body_defaults.max_tokens` folded, config §4.1). A `None` reaching encode for an Anthropic row is a config-resolution bug — `encode` returns `Error{kind: Config}` (→ exit 78) rather than omit it. |
| `messages` | array | `req.messages` | REQUIRED. Projection in §2.3. |
| `system` | string \| `array<TextBlockParam>` | `req.system: Option<Vec<Content>>` | **TOP-LEVEL, not a message.** Omit if `None`. §2.4. |
| `temperature` | float | `req.temperature` | omit if `None`. |
| `top_p` | float | `req.top_p` | omit if `None`. |
| `stop_sequences` | `array<string>` | `req.stop` | **RENAME:** canonical `stop` → wire `stop_sequences`. Omit if empty. ⚠ OpenAI uses `stop`; easy to confuse. |
| `stream` | bool | `req.stream.unwrap_or(false)` | emit `true` when streaming; an absent (`None`) stream is `false`. |
| `tools` | `array` | `req.tools` | omit if empty. §2.6. |
| `tool_choice` | object | `req.tool_choice` (+ `req.parallel_tool_calls`) | §2.7. May be omitted when `Auto` (the default). Carries the nested `disable_parallel_tool_use` knob. |
| *(merged)* `extra` | various | `req.extra` (`#[serde(flatten)]`) | merged at the **top level** — carries `thinking`, `metadata`, `service_tier`, `top_k`, `cache_control`, `container`, etc. §2.8. Typed fields win on a same-named key (§2.1.1). |

`top_k`, `thinking`, `metadata`, `service_tier` are **not** first-class canonical fields — they ride `req.extra` and merge at the top level (§2.8). **Per-block `cache_control` has NO canonical spelling at all**: it is placed AUTOMATICALLY by this encoder (§2.10) from the request's own shape, written BEFORE the `extra` fold so a policy marker WINS over any raw `cache_control` an `extra` key carries (§2.1.1, §2.8).

### 2.3 `messages[]` projection (the load-bearing part)

Each wire entry is `{ "role": "user"|"assistant", "content": string | array<ContentBlockParam> }`. Canonical `content` is ALWAYS `Vec<Content>` → project to an array of content blocks. **Collapse to a bare string only** when the vec is exactly one `Text` and carries no `cache_control`; the array form is always wire-equivalent and is the safe default (it never loses `cache_control`).

**Role projection** (`Role` → wire `role`), **owned entirely by this adapter** — the core never branches on Anthropic's tool convention:

| Canonical `Role` | Wire | Notes |
|---|---|---|
| `User` | `"user"` | |
| `Assistant` | `"assistant"` | |
| `System` | *(not emitted inline)* | **Hoisted** to the top-level `system` field, APPENDED after `req.system` in transcript order (§2.4). A `Message{role: System}` is NEVER written into `messages[]`, and NEVER dropped — `req.system` and `Role::System` are two distinct facts sharing this one wire home on Anthropic (architecture.md §3.1). (The wire's inline `"system"` role exists only under the `mid-conversation-system` beta — out of scope; a need for it is a CR.) |
| `Tool` | `"user"` | **Adapter projection.** Anthropic has NO tool role. A `Message{role: Tool, content: [ToolResult..]}` emits `{"role":"user","content":[{"type":"tool_result",…}]}`. Adjacent `Tool` messages may be emitted as consecutive `"user"` messages (the API combines same-role) — merging is optional, not required. |

**Placement invariant (a 400 if violated):** `tool_use` blocks belong in **assistant** messages; `tool_result` blocks belong in **user** messages (which is exactly where the `Role::Tool` projection puts them). `thinking`/`redacted_thinking` blocks, when present in an assistant turn, MUST come **first** in that turn's content array, before any `tool_use`/`text`, and MUST NOT be reordered or dropped (the API 400s otherwise — see §2.5).

**Cache-mark interaction (§2.10).** On an ongoing conversation the automatic rolling cache mark lands on the last eligible block of the last non-`assistant` **wire** message. Placement reads the already-projected wire array itself, so the `System` hoist and the §2.5 block drops are already applied — there is no canonical-index arithmetic to get wrong. A marked message NEVER collapses to a bare string (the array form is mandatory to carry the marker, §2.3 collapse rule).

### 2.4 `system` handling

The top-level `system` field is the ONE wire home for **all** canonical system text — both `req.system` AND every mid-transcript `Role::System` message hoist here (architecture.md §3.1: "Anthropic hoists either to its top-level `system`"; §2.3). The array is built in this order, and **nothing is dropped**:

1. every block of `req.system` (`Option<Vec<Content>>`), then
2. every block of each `Message{role: System}`, in transcript order.

- **Empty set → omit.** No `req.system` (`None` or `Some(vec![])`) AND no `Role::System` message → the built array is empty → omit `system` entirely. This is the no-system path, not a special case (architecture.md §3.1).
- Every block `Text` → emit `array<{type:"text","text":<s>}>`. Collapse to a bare string only if exactly one `Text` and no caching.
- **The automatic head cache mark (§2.10)** lands on the **last** block of the emitted `system` array (caching the tools+system prefix). An omitted (empty-set) `system` is the empty-set path — the mark falls to the last `tools` object (§2.6), never an error. A marked `system` always stays in array form (never the bare-string collapse, which cannot carry the marker).
- A non-`Text` Content (Image/ToolUse/ToolResult/Thinking/RedactedThinking) in **either** source (`req.system` **or** a `Role::System` message) → **UNREPRESENTABLE.** The wire `system` is a text-only slot. `encode` rejects with `Error{kind: ParseInput}` (→ exit 64) — the same **non-text-slot rejection rule** (architecture.md §3.1) for both sources. The canonical model stays a permissive `Vec<Content>` (single source of truth), and the adapter surfaces the text-only narrowing as a documented runtime degradation in this encode direction rather than silently dropping (§6 — resolved in architecture).

### 2.5 `Content` variant → ContentBlockParam

| Canonical `Content` | Wire block |
|---|---|
| `Text(String)` | `{"type":"text","text":<s>}` |
| `Image{source: Base64{media_type,data}}` | `{"type":"image","source":{"type":"base64","media_type":<mt>,"data":<b64>}}` — `media_type` ∈ {`image/jpeg`,`image/png`,`image/gif`,`image/webp`} only |
| `Image{source: Url{url}}` | `{"type":"image","source":{"type":"url","url":<u>}}` |
| `Document{source: Base64{media_type,data}}` | `{"type":"document","source":{"type":"base64","media_type":<mt>,"data":<b64>}}` — `media_type` `application/pdf` etc. Documents are **input-only**; no provider returns a `document` block, so there is no decode side (§6). |
| `Document{source: Url{url}}` | `{"type":"document","source":{"type":"url","url":<u>}}` — Anthropic fetches web URLs, so **both** document sources express here (unlike OpenAI Chat / Gemini / Ollama, which narrow — providers.md §9). The `text`/`file` source kinds are out until a need shows (architecture.md §3.1). |
| `ToolUse{id,name,input}` | `{"type":"tool_use","id":<id>,"name":<name>,"input":<input Value>}` — **assistant turn only** |
| `ToolResult{tool_use_id,content,is_error}` | `{"type":"tool_result","tool_use_id":<id>,"content":<string\|array<block>>,"is_error":<bool>}` — **inside a `"user"` message.** `content` is itself `Vec<Content>` → array of text/image blocks (or bare string if a single `Text`). The wire `tool_result` content is a **text/image-only slot**: a non-(`Text`\|`Image`) `Content` nested here (`ToolUse`/`ToolResult`/`Thinking`/`RedactedThinking`) is **UNREPRESENTABLE**, and `encode` rejects with `Error{kind: ParseInput}` (→ exit 64), the same non-text-slot rejection as §2.4 (architecture.md §3.1). `is_error` may be omitted when `false`. |
| `Thinking{text, signature: Some(sig)}` | `{"type":"thinking","thinking":<text>,"signature":<sig>}` — **`signature` passed VERBATIM**, never modified/omitted. |
| `Thinking{text, signature: None}` | A thinking block with no signature **cannot be replayed to Anthropic** (the API rejects a thinking block whose signature is absent on a multi-turn replay). On encode this is a representational gap; see **CR-2** (§6). v0.1 behavior: omit the block (do not fabricate a signature, do not send a signature-less thinking block that would 400). |
| `RedactedThinking{data}` | `{"type":"redacted_thinking","data":<data>}` — **`data` passed VERBATIM**, never modified/reordered/dropped. Maps cleanly to the `Content::RedactedThinking{data}` variant (architecture.md §3.1); like `thinking`, it MUST come first in the assistant turn (§2.3). |
| `ServerToolUse{id,name,input}` | `{"type":"server_tool_use","id":<id>,"name":<name>,"input":<input Value>}` — **VERBATIM** (CR-4 resolved). A provider-executed invocation echoed back on replay untouched; **never folded into `tool_use`** (converting it makes the API demand a nonexistent client `tool_result` and 400 — the litellm-class replay corruption). id uses `srvtoolu_`. |
| `ServerToolResult{kind,tool_use_id,content}` | `{"type":<kind>,"tool_use_id":<id>,"content":<content Value>}` — **VERBATIM** (CR-4 resolved). `kind` IS the wire tag (`web_search_tool_result`, `code_execution_tool_result`, …): an **open set carried as data**, suffix-matched at decode (`ends_with("_tool_result")` && ≠ `tool_result`) and re-emitted untouched, so the whole family round-trips with zero per-tool knowledge (the `model` open-set rule applied to result blocks). `content` is the opaque provider payload — an array of results on success, or a `{type:*_tool_result_error,...}` OBJECT on failure. **Never folded into a client `tool_result`.** |

Both `thinking`/`redacted_thinking` block kinds map cleanly to their own canonical `Content` variants now — there is no lossy folding of `redacted_thinking` into `Thinking` (that earlier interim hack is dropped; see §6 — resolved in architecture). The two server-tool variants follow the same verbatim round-trip rule.

### 2.6 `tools` projection

Canonical `Tool` is a two-variant enum (architecture.md §3.1, CR-4 resolved), each with its own wire shape:

- **`Tool::Custom{name, description: Option, input_schema: Value, strict: Option<bool>}`** → the flat custom-tool object:

```json
{"name":<name>, "description":<desc?>, "input_schema":<JSON-Schema object>, "strict":<strict?>}
```

No `type` field for custom tools (the wire defaults to `"custom"`). `input_schema` must be a JSON Schema with `"type":"object"`. `description` is omitted when `None`. `strict` (the lifted per-tool strict-tool-use knob, architecture.md §3.1) folds top-level onto the tool object when `Some`, else omitted (byte-stable with the pre-knob wire).

- **`Tool::Provider{kind, name, config}`** → the provider-typed object (Anthropic-schema client tools bash/computer/… AND server tools web_search/…):

```json
{"type":<kind>, "name":<name>, ...config}
```

`kind` is the verbatim wire `type` (`web_search_20250305`, `bash_20250124`, …) — opaque to brazen, the provider is the authority (a bad one → provider 400). Every `config` key (max_uses, allowed_domains, user_location, …) folds in verbatim; **no `input_schema`, no `description`**. Any required `anthropic-beta` header is the caller's job via the row's `beta_headers` data — brazen ships no version→beta tables.

When `system` is absent/empty, the **automatic head cache mark (§2.10)** lands on the **last** object in the emitted `tools` array (caching the tool-definitions prefix). Both absent/empty → no mark (the empty-set rule), never an error.

### 2.7 `tool_choice` mapping

| Canonical `ToolChoice` | Wire |
|---|---|
| `Auto` | `{"type":"auto"}` — may be **omitted entirely** when there are no tools (the wire default) |
| `Any` | `{"type":"any"}` — (canonically the same intent as OpenAI's `"required"`) |
| `Tool{name}` | `{"type":"tool","name":<name>}` |
| `None` | `{"type":"none"}` |

**`disable_parallel_tool_use` ← `req.parallel_tool_calls`.** Anthropic nests the
parallel-tool-calls knob *inside* the `tool_choice` object, so it CANNOT ride the
top-level `extra` valve (§2.8 is a shallow top-level merge — a key placed there
lands at the body top level, which the API rejects). It is therefore the canonical
typed field `parallel_tool_calls: Option<bool>` (a lifted known knob, architecture.md
§3.1 — OpenAI spells the same intent as a top-level `parallel_tool_calls`), projected
here into the `tool_choice` object:

- `Some(false)` → add `"disable_parallel_tool_use": true` — **but only onto an `auto` or
  `any` `tool_choice`.** Those are the two choices where more than one tool call is
  possible, so "disable parallel" is meaningful; Anthropic documents the field on them.
- `Some(true)` / `None` → omit (Anthropic's default is parallel-enabled).

**The fold is RESTRICTED to `auto`/`any`.** On `{"type":"none"}` (no call is made) and
`{"type":"tool","name":…}` (exactly one, forced call) there is no parallelism to disable —
the field is undocumented and nonsensical there — so `disable_parallel_tool_use` is NOT
added. With those two choices `parallel_tool_calls: false` is therefore **inexpressible on
the Anthropic wire and DROPS** (it stays on the canonical request for every other
protocol; the knob was already a no-op given a forced/suppressed single call). This is the
one home for the complete `tool_choice` projection (`tool_choice_value`), so the fold and
this restriction live together, not at the call sites (shared by `encode` and the
`count_tokens` body, §2.11). The fold happens only when a `tool_choice` object is emitted
at all; `Auto` with no tools omits `tool_choice` entirely, so the knob is a no-op there too.

### 2.8 `extra` passthrough (Anthropic-specific knobs)

`req.extra` is merged at the **top level** of the body (typed fields win, §2.1.1). It carries everything Anthropic-specific that the canonical struct does not model:

- **`thinking`**: `{"type":"adaptive","display":"summarized"|"omitted"}` (Opus/Sonnet 4.6+) · `{"type":"enabled","budget_tokens":N}` (older models) · `{"type":"disabled"}`.
- **`metadata`**: `{"user_id": <string>}`.
- **`service_tier`**: `"auto"|"standard_only"`.
- **`top_k`**: int.
- **`container`**, etc. (`disable_parallel_tool_use` is **not** here — it nests in `tool_choice`; see §2.7.)

`encode` performs a **shallow top-level merge**: it serializes the typed fields first, then folds in `extra` keys not already set by a typed field (§2.1.1). This is the severability valve of architecture.md §2 / §4.1: a new Anthropic knob needs **zero code**, only an `extra` key. Note `cache_control` is NOT reachable through this valve: it is per-block wire data inside the typed `system`/`tools`/`messages` arrays, which `extra` cannot override (§2.1.1) — the automatic §2.10 marks are the one source of `cache_control` on the encoded path (`--raw` is the escape).

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
  "system": [{"type": "text", "text": "You are a terse weather bot.",
              "cache_control": {"type": "ephemeral"}}],
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
       "content": [{"type": "text", "text": "62F, foggy"}],
       "cache_control": {"type": "ephemeral"}}
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

The five reframes to notice in this example:
1. `system` is hoisted to the **top level** (not a message); the `Role::System` content never appears in `messages[]`.
2. `Role::Tool` projected to a `"user"` message carrying a `tool_result` block.
3. canonical `stop` → wire `stop_sequences` (the rename), and `Thinking` placed **first** in the assistant turn with its `signature` byte-for-byte.
4. `thinking` (from `extra`) merged at the top level alongside the typed fields.
5. the two `cache_control` marks are AUTOMATIC (§2.10) — nothing in the canonical request asked for them: the head mark on the last `system` block, and (this being an ongoing conversation — an assistant turn precedes the last message) the rolling mark on the last block of the last non-assistant wire message.

### 2.10 Prompt caching — automatic `cache_control` placement (zero canonical surface)

Prompt caching is **brazen-owned policy with no canonical surface**: no request field, no flag, no config key (architecture.md §2/§3.1 — the harness knows only brazen, never a provider's cache dialect; the typed `req.cache` breakpoint surface that briefly existed pre-0.1 is deleted). This encoder decides `cache_control` placement from **the request's own shape**, computing marks on the **already-built** `body` arrays (the SSOT for projection — the §2.3 System hoist and §2.5 block drops are already applied), written BEFORE the `extra` fold so a policy marker wins over any raw `cache_control` an `extra` key carries (§2.1.1, §2.8). The caller observes the outcome only through the response-side `Usage.cache_read_tokens`/`cache_write_tokens` (§3.6).

The policy — placement is MECHANISM, tunable later with no interface change:

1. **Head mark — always.** `cache_control: {"type":"ephemeral"}` on the **last** `system` wire block (caching the tools+system prefix, §2.4); if `system` is empty/absent, on the **last** `tools` object (§2.6); if both are empty, no mark (the empty-set rule). Sub-minimum prefixes (1024/4096 tokens by model) are Anthropic's documented **silent no-op** — never brazen's to police — so the unconditional mark costs a one-shot nothing (small head) or at most one 25% write premium (big head, never recurs).
2. **Rolling conversation mark — when the request is an ongoing conversation.** Trigger: at least one `assistant` message **strictly before** the last wire message. (A lone TRAILING assistant is a prefill one-shot — no trigger: a prefill is extended by the completion, so its blocks mutate and must not anchor a cache.) Placement: the last **eligible** wire block of the **last non-`assistant`** wire message; a message with 0 eligible blocks walks back to an earlier message (earlier assistant turns are stable history, so the walk crosses them freely; nothing eligible anywhere → no mark).
3. **One intermediate mark** exactly 20 eligible wire blocks before the rolling mark, only when that many eligible blocks precede it — this keeps the PREVIOUS turn's write point inside Anthropic's ~20-block cache-hit lookback even when a turn's delta is large. Total marks ≤ 3 by construction (head + intermediate + rolling), so the provider's 4-marker cap is unreachable and **no validation error exists on this path at all**.
4. **Eligibility:** `cache_control` is invalid on `thinking`/`redacted_thinking` blocks — when the natural target is one, the mark steps back to the previous eligible block.
5. **TTL: always omitted** → `{"type":"ephemeral"}` (Anthropic's 5-minute default). The 5m entry **renews on every cache read**, so a steady agent loop stays warm indefinitely; `1h` only wins across idle gaps a stateless adapter cannot see. Not exposed anywhere.

**Steady state.** In a loop that re-feeds the growing transcript, from turn 2 onward the same marks land at the same prefixes, so every turn reads the previous turn's write — the heuristic reaches the same steady state an explicit breakpoint interface would, and differs only on non-recurring multi-turn requests.

**Sharp edge (deliberate, documented).** Non-recurring REPLAYED conversations (evals, batch replays over `bz`) pay a one-time 25% cache-write premium on marked prefixes. The escape today is `--raw` (provider-native bytes, no policy); a typed opt-out knob is ADDITIVE later if real usage demonstrates the need.

**Recorded non-goals:**
- **An XDG prefix-hash journal** (observing cross-invocation recurrence for precise placement) is the upgrade path if the stateless heuristic's misses ever matter in practice. If ever built: hashes only (never content), reaped on every invocation. Not now.
- **An explicit intent field** (e.g. `CacheExtent{Head, Through{message}}`): additive later if an embedder demonstrates a need for explicit control.
- **Google `CachedContent`**: a stateful out-of-band handle; violates one-round-trip-per-process.

### 2.11 Token counting — `POST /v1/messages/count_tokens` (the `--count-tokens` control op)

The `--count-tokens` control op (architecture.md §5.10.1, bl-24e5) does ONE round-trip to Anthropic's count endpoint and returns the provider-accurate input-token count. This impl supplies it as DATA on the protocol (`Protocol::count_tokens`, the sibling of `models_shape()`) — the sole dialect knowledge is the endpoint path and the response key; the body is **this encoder's own message/system/tool projection**, so the counted request is byte-for-byte the shape §2 would send, minus the fields the count endpoint does not accept.

- **Endpoint:** `POST {base_url}/v1/messages/count_tokens` (the `/v1/messages` path plus `/count_tokens`), `content-type: application/json`, and the SAME auth + `anthropic-version` (and any beta) headers the generation request and the `--list-models` GET carry — stamped by the count runner after this impl builds the body, exactly as `serve` stamps them for `encode` (§2.1). No new auth path.
- **Request body — the §2 body MINUS the generation-only fields.** The count endpoint validates a `MessageCountTokensParams`, which accepts `model`, `messages`, `system`, `tools`, `tool_choice`, and `thinking` (each projected by the SAME §2.3/§2.4/§2.5/§2.6/§2.7 helpers and the §6 reasoning→`thinking` mapping) — and **rejects** `max_tokens`, `stream`, `temperature`, `top_p`, and `stop_sequences` (generation controls that do not affect the input-token count). So `count` reuses the projection leaves but omits those five keys; it does **not** call `encode` and re-strip, because that would parse-and-rewrite the serialized body — instead the shared leaves (`system_value`/`messages_value`/`tools_value`/`tool_choice_value`) are called directly, keeping `encode`'s own byte output untouched. Automatic `cache_control` placement (§2.10) is applied for fidelity — the count reflects the exact prefix the data plane would cache; the endpoint accepts `cache_control` and it does not change the returned total. The `extra` passthrough (§2.8) folds beneath the typed keys as it does in `encode`.
- **Response:** `{"input_tokens": N}` (2xx). The impl names the response key `input_tokens`; the count runner projects it to the canonical `{"input_tokens": N}` / bare `N` output. A non-2xx routes through the ONE `http_error` home (§4.3) exactly like the generation path (401/403 → 77, other 4xx → 69, 5xx → 70). A well-formed 2xx whose body carries no `input_tokens` number is a `Provider{502}` (an upstream contract violation, the count analog of §3.1's malformed models list).
- **No retry, no cache write.** The op reads the per-provider model cache to resolve the seed to a wire id (`select_model`, model-discovery §5.2) — the same resolution the data plane runs — but never writes it (it is not the discovery path). One round-trip.

### 2.12 `output_config` — the structured-output (`req.output`) projection

`output: Option<OutputFormat>` (architecture.md §3.1) → the top-level `output_config` (`None` → omit). Anthropic's structured-output wire is **GA (no beta header)** and **SCHEMA-ONLY** — it has no schemaless JSON mode and no `name`/`strict` field (structured outputs is inherently strict):

- `OutputFormat::JsonSchema{schema, ..}` → `output_config: {"format": {"type":"json_schema", "schema":<schema>}}`. `name`/`strict` are **narrowed** (no Anthropic field).
- `OutputFormat::Json` (schemaless JSON mode) → **OMITTED**. Anthropic's wire genuinely LACKS a schemaless mode, so this is a documented narrowing (CR-R1, the same class as `stop` on Responses, providers §6.1) — never a mistranslation. A caller needing constrained JSON on Anthropic supplies a `JsonSchema`.

Written **before** the `cache_control` placement and the `extra` fold (§2.1.1), so a typed `output` WINS over a raw `output_config` an `extra`/`body_defaults` key carries. Incompatible with the strict-tool-use `strict` only per Anthropic's own API rules (not brazen's concern — the provider enforces). `output_config` is a body fact, so `--raw` bypasses it entirely.

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

`ping` (`data: {"type":"ping"}`) may appear anywhere (keep-alive → zero events). A mid-stream `error` event may appear after the HTTP 200 (§3.4 / §4.2); it is **terminal and is NOT followed by `message_stop`** — the wire closes the stream after it.

### 3.2 `DecodeState` (caller-owned cross-frame state)

The impl is pure over `(frame, &mut state)`; everything that must survive across frames lives here, NOT on the impl:

```rust
// fields this protocol requires of the shared DecodeState (the SSE-decoder spec owns the type)
struct DecodeState {
    open: HashMap<u32, OpenBlock>,   // index -> in-flight block, opened at content_block_start, removed at content_block_stop
    terminated: bool,                // set true when decode consumes the provider terminal marker (message_stop); gates run's premature-EOF injection (architecture.md §5.6, CR-9)
    // (cumulative usage is re-emitted as revealed; no accumulation needed beyond
    //  what the wire already states cumulatively — see §3.6)
    // ...shared fields from the SSE-decoder spec (SSE buffering, etc.)
}

struct OpenBlock {
    kind: BlockKind,           // Text | ToolUse | Thinking | RedactedThinking — its IDENTITY
}
```

`OpenBlock` carries ONLY the block `kind` (its identity): fragments are emitted DIRECTLY as `ContentDelta`s the moment they arrive, never accumulated in the block (the canonical sink folds them, architecture.md §5). The block exists only to route a delta to its index and to synthesize the block's `ContentStop` at close — there is no per-block `json`/`signature`/`data` buffer parsed at fold time. `open` is keyed by the wire `index`, which is the single source of truth for "which block a delta routes to." A block is inserted at `content_block_start` and removed at `content_block_stop`. `terminated` is the one bit architecture.md §5.6 reads to distinguish a clean end (a consumed terminal marker) from a premature EOF (CR-9, resolved in architecture).

### 3.3 How the ContentStart-before-deltas invariant is met

**Natively.** Anthropic's `content_block_start` carries the block's full identity *before any delta bytes* — for `tool_use`, the `id` and `name` are present at start (`input` is always `{}` at start; real args arrive as deltas). So `decode` emits `ContentStart{index, kind}` — with `kind: ContentKind::ToolUse{id, name}` already populated — the moment it sees `content_block_start`, and only then can any `ContentDelta{index, …}` for that index follow. No "have I seen the id yet?" branch is ever needed; the triple `(ContentStart, ContentDelta*, ContentStop)` is identical downstream to the OpenAI adapter's *synthesized* equivalent (architecture.md §3.6).

### 3.4 Event-by-event mapping

#### `message_start` → `MessageStart{id, model, role}` + (if present) `Usage`

```json
{"type":"message_start","message":{"id":"msg_…","role":"assistant","model":"claude-opus-4-8","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":25,"output_tokens":1}}}
```

- `message.id` → `MessageStart.id = Some(..)`; `message.model` → `MessageStart.model = Some(..)`; `message.role` is always `"assistant"` → `Role::Assistant`. (`MessageStart.v` is the constant event-schema version stamped by `Event::message_start`, not wire-derived — architecture.md §3.2.)
- `message.usage`, **if present**, → a `Usage` event (see §3.6 for field mapping). The usage object is **OPTIONAL** on `message_start` (in the thinking example it is absent entirely) — emit `Usage` only when present; never fabricate `0`.

Emits `[MessageStart, Usage?]` (the `Usage` only if `message.usage` exists). This initial `Usage` is the first cumulative snapshot; a later `message_delta` `Usage` **supersedes** it field-by-field (a folding consumer keeps last-wins per field, §3.6).

#### `content_block_start` → `ContentStart{index, kind}`

The nested `content_block.type` selects `ContentKind`; `index` (u32) is the open-block key.

| wire `content_block.type` | `ContentKind` | DecodeState action |
|---|---|---|
| `text` (`{"type":"text","text":""}`) | `Text {}` | insert `OpenBlock{kind:Text}`. (The wire's start `text` field is **always the empty string** — there is no text seed to emit; real text arrives as `text_delta`.) |
| `tool_use` (`{"type":"tool_use","id":"toolu_…","name":"get_weather","input":{}}`) | `ToolUse { id, name }` | insert `OpenBlock{kind:ToolUse}`. **Identity is native here.** `input` is always `{}`; ignore it (real args arrive as `input_json_delta`s, emitted DIRECTLY — never buffered on the block). |
| `thinking` (`{"type":"thinking","thinking":"","signature":""}`) | `Thinking { id: None }` | insert `OpenBlock{kind:Thinking}`. `thinking`/`signature` start empty; Anthropic has no reasoning-item id, so `id` is always `None` (renders `"thinking":{}` — byte-identical to the pre-bl-61a9 shape). |
| `redacted_thinking` (`{"type":"redacted_thinking","data":"<opaque>"}`) | `RedactedThinking { data }` | insert `OpenBlock{kind:RedactedThinking}`. The `data` blob is present **at start** and is now CARRIED INLINE on `ContentKind::RedactedThinking { data }` (bl-61a9, resolving CR-5's dropped-data gap) — mirroring `ServerToolResult`'s inline-content-at-start, so a `--json` harness can rebuild the block. No streamed delta follows. |
| `server_tool_use` (`{"type":"server_tool_use","id":"srvtoolu_…","name":"web_search","input":{}}`) | `ServerToolUse { id, name }` | insert `OpenBlock{kind:ServerToolUse}` (CR-4 resolved). **Tracked exactly like `tool_use`**: identity at start, the input arrives as `input_json_delta`s → `JsonDelta`, emitted DIRECTLY. |
| `*_tool_result` (suffix rule: any tag `ends_with("_tool_result")` except the client `tool_result`) | `ServerToolResult { kind, tool_use_id, content }` | insert `OpenBlock{kind:ServerToolResult}` (CR-4 resolved). `kind` carries the wire tag verbatim (open set — `web_search_tool_result`, `code_execution_tool_result`, …); the **full `content` Value arrives INLINE at start** (array of results, or the `{type:*_error,...}` object) — **no delta ever follows**; the block then closes on its `content_block_stop`. |

Emits `[ContentStart{index, kind}]`. On the wire the externally-tagged `ContentKind` renders `"kind":{"text":{}}`, `"kind":{"thinking":{}}`, `"kind":{"redacted_thinking":{"data":"<opaque>"}}`, or `"kind":{"tool_use":{"id":…,"name":…}}` (architecture.md §3.2, §5.2).

#### `content_block_delta` → `ContentDelta{index, delta}` (or DecodeState mutation)

`delta.type` selects the variant; `index` routes to the open block.

| wire `delta.type` | canonical | action |
|---|---|---|
| `text_delta` (`{"type":"text_delta","text":"Hello"}`) | `Delta::TextDelta(text)` | emit `ContentDelta{index, TextDelta}`. |
| `input_json_delta` (`{"type":"input_json_delta","partial_json":"{\"loc"}`) | `Delta::JsonDelta(partial_json)` | emit `ContentDelta{index, JsonDelta}` DIRECTLY — the fragment is never buffered on the block. Fragments are valid **only concatenated**; the first is frequently `""`; a fragment may split mid-UTF-8 or mid-JSON-number. **NEVER parse mid-stream** (the canonical sink concatenates and parses at fold, architecture.md §5). |
| `thinking_delta` (`{"type":"thinking_delta","thinking":"…"}`) | `Delta::ThinkingDelta(thinking)` | emit `ContentDelta{index, ThinkingDelta}`. |
| `signature_delta` (`{"type":"signature_delta","signature":"EqQB…"}`) | `Delta::SignatureDelta(signature)` | emit `ContentDelta{index, SignatureDelta}` (bl-61a9, resolving CR-5). Arrives exactly once, immediately before `content_block_stop` of a thinking block — wire-arrival order is preserved. With `display:"omitted"` the thinking block gets ONLY this (zero `thinking_delta`). A sink folds it onto the block's `Content::Thinking.signature`. |

For `text_delta`/`input_json_delta`/`thinking_delta`/`signature_delta`: emits `[ContentDelta{…}]` DIRECTLY, the moment the fragment arrives (never buffered on the block). The externally-tagged `Delta` renders `"delta":{"text_delta":"Hel"}`, `"delta":{"json_delta":"…"}`, `"delta":{"thinking_delta":"…"}`, `"delta":{"signature_delta":"EqQB…"}` (architecture.md §3.2, §5.2). A `redacted_thinking` block carries **no** delta at all — its `data` rode `ContentStart` (§3.4 above).

#### `content_block_stop` → `ContentStop{index}`

```json
{"type":"content_block_stop","index":0}
```

Closes the block at `index`: remove `open[index]` and emit `[ContentStop{index}]`. (The folded `Content` value — `ToolUse{input: JSON.parse(<concatenated JsonDelta fragments>)}` or `Thinking{text}` from the concatenated `ThinkingDelta`s — is materialized **only** when a consumer folds the event stream to `Content`; `decode` itself emits just the `ContentStop` marker. The fragments are the source — they ride the DIRECTLY-emitted `ContentDelta` events, NOT a per-block `DecodeState` buffer; the concatenate-and-parse happens at fold time per architecture.md §3.5/§3.6/§5. A harness rebuilds `Content::ServerToolUse` the same way — accumulate the `JsonDelta`s into `input` — and `Content::ServerToolResult` by taking `kind`+`content` straight off its `ContentStart`, nothing to accumulate.) Every content block — including the `server_tool_use` block in `anthropic_messages_pause` (which now SURFACES, CR-4 resolved) — emits its `content_block_stop` before the terminal `message_delta`, so the universal "every `ContentDelta.index` has a following `ContentStop`" invariant holds on the normal and pause paths (§3.10).

#### `message_delta` → `Finish{reason}` (if terminal) + `Usage`

```json
{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}
```

- May appear **more than once**. `delta.stop_reason` is `null` on intermediate events; **emit `Finish` only when `stop_reason` is non-null** (the terminal one). Mapping in §3.5.
- `usage` here is **cumulative** and authoritative for the final `output_tokens` (and may restate input/cache fields) → a `Usage` event (§3.6). It supersedes the `message_start` `Usage` field-by-field for a folding consumer (last-wins).

Emits `[Usage?, Finish?]` — `Usage` whenever a `usage` object is present, `Finish` only when `stop_reason` is non-null. (A `Finish{reason}` arrives **before** `End` but they are distinct: `Finish` is *why* generation stopped; `End` is *the byte stream is over* — architecture.md §5.6.)

#### `message_stop` → nothing (the terminator is `run`-owned)

```json
{"type":"message_stop"}
```

→ emits `[]` **and sets `state.terminated = true`** (this is the Anthropic terminal marker, architecture.md §5.6 / CR-9). `decode` does **not** emit `End`; the single `Event::End` is appended by the `run` loop at body EOF (architecture.md §4.4, §3.8). This is the native terminator that *causes* the upstream to close, which is what triggers that EOF; because `terminated` is set, `run` does **not** inject a premature-EOF error.

#### `ping` → nothing

`{"type":"ping"}` → emits `[]`. Pure keep-alive.

#### `error` (mid-stream, after HTTP 200) → `Error(CanonicalError)`

See §4.2 — a terminal stream error, not folded into `Finish`. `decode` emits `[Error(..)]` with the `kind` mapped per §4.2; it does **not** emit `End` (§3.8). A decoded `Event::Error` is itself a clean terminal for the premature-EOF rule (architecture.md §5.6).

### 3.5 `stop_reason` → `FinishReason`

| wire `stop_reason` | `FinishReason` |
|---|---|
| `"end_turn"` | `Stop` |
| `"max_tokens"` | `Length` |
| `"stop_sequence"` | `StopSequence` (the matched string in top-level `stop_sequence` / `delta.stop_sequence` is **dropped** — `StopSequence` carries no payload; see CR-6) |
| `"tool_use"` | `ToolUse` |
| `"pause_turn"` | `Pause` (server-tool sampling loop hit its iteration limit — only appears with server tools; resume by re-sending the assistant content as-is) |
| `"refusal"` | `Refusal{category, explanation}` — see §3.7 |
| `"model_context_window_exceeded"` | `Other("model_context_window_exceeded")` — no dedicated variant; do NOT conflate with `Length` (max_tokens). See **CR-7** (§6). |
| any unknown | `Other(s)` — never panics (architecture.md §9.5) |

`StopSequence` is **Anthropic-specific** — the sibling OpenAI mapping (openai-chat-mapping.md §3.5) reports a stop-sequence hit as `Stop`, so a stop-sequence cross-check pairing would decode to different `FinishReason`s and is **excluded** from the cross-check equality (§5.1).

### 3.6 `Usage` field mapping

Anthropic usage objects (on `message_start.usage` and the final `message_delta.usage`; NOT every `message_delta`) map field-by-field; every canonical field is `Option`, `None` when the wire omits it (never `0`):

| wire usage field | canonical `Usage` field |
|---|---|
| `input_tokens` | `input_tokens` (the **uncached** remainder; total prompt = `input_tokens` + `cache_write_tokens` + `cache_read_tokens`) |
| `output_tokens` | `output_tokens` (grows over the stream; the terminal `message_delta` is authoritative) |
| `cache_creation_input_tokens` | `cache_write_tokens` |
| `cache_read_input_tokens` | `cache_read_tokens` |
| `server_tool_use.web_search_requests` | *(no canonical field — the ONE remaining deferred piece of CR-4, §6; ignored / rides provider_detail. The content BLOCKS are resolved; this usage COUNTER is not.)* |

Usage is **cumulative**; emit a `Usage` event whenever a usage object is revealed. The `message_start` `Usage` is the **initial cumulative snapshot**; the terminal `message_delta` `Usage` **supersedes it field-by-field** (the typical wire restates only `output_tokens` on `message_delta`, leaving `input_tokens`/cache fields `None` there), so a folding consumer keeps **last-wins per field** to get the authoritative final totals. The initial `message_start` `output_tokens` is typically 1–3.

### 3.7 Refusal → `Finish{Refusal}` (HTTP 200, exit 0 — NEVER an Error)

A refusal arrives as **HTTP 200** with `stop_reason: "refusal"` and `content: []` (pre-output) — folded to `Finish{Refusal{category, explanation}}`, exit 0. Non-stream body shape:

```json
{"id":"msg_…","role":"assistant","content":[],"stop_reason":"refusal","stop_sequence":null,
 "stop_details":{"category":"cyber","explanation":"…"},"usage":{"input_tokens":100,"output_tokens":5}}
```

- `stop_details.category` (string, e.g. `"cyber"`, `"bio"`, may be `null`) → `Refusal.category` (`""` if `null`/absent).
- `stop_details.explanation` (string|null) → `Refusal.explanation` (`Option`).
- `stop_details` is `null`/absent for all other `stop_reason`s.

**Streaming refusal:** arrives as `message_delta` with `delta.stop_reason: "refusal"`. ⚠ Whether `stop_details` rides the streamed `message_delta` is **UNVERIFIED against a live capture**; see **CR-8** (§6) and the `anthropic_messages_refusal` fixture (§5), which MUST be recorded from a real streamed refusal to settle this. If `stop_details` is absent in-stream, decode emits `Finish{Refusal{category:"", explanation:None}}`. **Never** an `Error`, always exit 0.

### 3.8 The one terminator → exactly one `End` (and the §5.6 interaction)

`decode` **never emits `Event::End`.** Both terminal wire events stop producing events:

- `message_stop` → `[]`, and sets `state.terminated = true`. It is the native terminator; the upstream then closes, the body iterator reaches EOF, and `run` writes the **one** `End` (architecture.md §4.4).
- mid-stream `error` (no `message_stop` follows) → `[Error(..)]`. The upstream closes, EOF is reached, and `run` writes the **same one** `End`.

This yields exactly one `End` in both the normal and error-terminated cases — End ownership is a structural property of `run`. It is identical to the sibling OpenAI chat mapping (openai-chat-mapping.md) §3.6 (`[DONE]` → `[]` + `terminated`, `End` `run`-appended), so the two protocols share one terminator projection.

**Interaction with architecture.md §5.6 (premature upstream EOF) — resolved in architecture (CR-9).** architecture.md §5.6 injects an in-band `Event::Error{kind:Transport}` then `Event::End` then exit 69 on a *premature* upstream EOF, but a **clean** stream also ends in EOF. The architecture resolves the ambiguity with the `DecodeState.terminated` bit: `run` injects the premature-EOF `Error{Transport}` + exit 69 **only if `!terminated`**. Because `message_stop` sets `terminated = true`, a clean Anthropic end suppresses the injection. A mid-stream `error` is likewise a clean terminal (a decoded `Event::Error`), so the injection does not fire and there is no double-error. `decode` stays pure — it sets the one bit and never emits `End`; `run` owns the single `End` unconditionally. The same coordination applies to the sibling OpenAI mapping (a non-2xx error body is likewise a clean, terminal end).

### 3.9 Worked RESPONSE trace (SSE → NDJSON)

A "basic text" streamed response, **with usage** (the §5 `anthropic_messages_usage`-style trace; the cross-check `anthropic_messages_basic` fixture is the same shape with the two `Usage` events normalized out, §5.1). **Input SSE**:

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

**Decoded `Vec<Event>`** (one decode call per frame; `[]` for `ping` and `message_stop` — the latter also sets `terminated`):

```
MessageStart{id:Some("msg_01XYZ"), model:Some("claude-opus-4-8"), role:Assistant}
Usage{input_tokens:Some(12), output_tokens:Some(1), cache_read_tokens:None, cache_write_tokens:None}
ContentStart{index:0, kind:Text}
ContentDelta{index:0, delta:TextDelta("Hel")}
ContentDelta{index:0, delta:TextDelta("lo")}
ContentStop{index:0}
Usage{input_tokens:None, output_tokens:Some(2), cache_read_tokens:None, cache_write_tokens:None}
Finish{reason:Stop}
End                                  // appended once by run() after body EOF (§3.8); not injected as premature because terminated==true
```

**NDJSON on stdout** (architecture.md §5.2 — one Event per line, flushed, ending in `{"type":"end"}`; `ContentKind`/`Delta` render externally-tagged per architecture.md §3.2):

```
{"type":"message_start","id":"msg_01XYZ","model":"claude-opus-4-8","role":"assistant"}
{"type":"usage","input_tokens":12,"output_tokens":1,"cache_read_tokens":null,"cache_write_tokens":null}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}
{"type":"content_stop","index":0}
{"type":"usage","input_tokens":null,"output_tokens":2,"cache_read_tokens":null,"cache_write_tokens":null}
{"type":"finish","reason":"stop"}
{"type":"end"}
```

The `"kind":{"text":{}}` and `"delta":{"text_delta":"Hel"}` shapes are the **externally-tagged** rendering of `ContentKind`/`Delta` (architecture.md §3.2, §5.2); `Event` keeps its `"type"` internal-tag envelope.

This decode is **deterministic under adversarial rechunking** (architecture.md §9.3): feeding the same SSE bytes through `OneByte`, `MidData`, `MidUtf8`, `MidJsonNumber`, `WholeFixture` yields this exact `Vec<Event>`. `MidUtf8`/`MidJsonNumber` exercise the `SseDecoder`'s partial-frame buffering (the SSE-decoder spec); `decode` only ever sees a complete frame.

### 3.10 Open blocks at terminal events

On a terminal `message_delta` (non-null `stop_reason`) or a mid-stream `error`, any block still in `state.open` was opened by a `content_block_start` with no matching `content_block_stop`. On the **normal** Anthropic wire this does not happen — every block is closed by its own `content_block_stop` before the terminal `message_delta` (including the `server_tool_use` block on the `pause_turn` path, §3.4 / `anthropic_messages_pause`). The universal "every `ContentDelta.index` has a following `ContentStop`" invariant (architecture.md §9.2) is therefore satisfied by the wire, not synthesized here. On a **truncated** stream (premature EOF, no `message_stop` → `terminated` stays `false`), `run`'s §5.6 path produces `Error{Transport} + End` and the partial event vector is accepted as-is — the invariant is **explicitly scoped to exclude truncated/error-terminated streams**, which is the only case an open block survives to EOF (§3.8).

---

## 4. ERROR mapping (HTTP status + body → `CanonicalError` + exit code)

Per architecture.md §8: every failure → `Event::Error(CanonicalError{kind, message, provider_detail})` AND a computed exit code. **The HTTP status is peeked separately from the body** (`TransportResponse.status`, architecture.md §4.1) and drives the exit code for a failed handshake; `decode` parses the body (pure, fixture-tested, no network) for `message`/`provider_detail`/`kind`. For an **in-band mid-stream error** (§4.2) the exit comes from the decoded `kind` directly, NOT the 2xx handshake status (architecture.md §8 / CR-10).

### 4.0 How a non-SSE error body reaches `decode` (the SSE-decoder contract)

A non-2xx Anthropic error response is a bare JSON object, not an SSE stream — the SSE frame grammar would never yield a frame from it. The bridge is owned by the SSE-decoder spec and named here so §4.1's parse is reachable:

> **SSE-decoder contract (shared with the OpenAI chat mapping (openai-chat-mapping.md) §4.0):** when `TransportResponse.status` is **non-2xx**, the `run` loop / SSE decoder does **not** apply SSE framing; it hands `decode` the **whole response body as a single `Frame`** carrying that status as **`frame.status: Some(code)`** (sse-decoder §9). `decode` recognizes the whole-body error frame by `frame.status.is_some()`, derives `kind` from the carried status (§4.3), and parses the body for `message`/`provider_detail` (§4.1). The carried status is the same status `run` peeks for the exit code — read by `decode`, never reconstructed from the body.

A **mid-stream `error` SSE event** (§4.2) is different: it arrives on a **2xx** stream as a normal SSE frame (`event: error` / `data: {...}`) and is dispatched by `data.type` like any other frame. Both paths end at the same `Event::Error` construction (§4.1); they differ only in how `kind` is derived (§4.2 for in-band, §4.3 for HTTP).

### 4.1 The error body shape (parsed in decode)

Anthropic's error envelope is identical for HTTP errors and mid-stream SSE `error` events:

```json
{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},
 "request_id":"req_011CSHo…"}
```

- For a **mid-stream** SSE `error` event (§4.2): `error.message` → `CanonicalError.message`, the full `error` object → `CanonicalError.provider_detail` (verbatim `Value`), and `error.type` → `kind` (there is no governing status).
- For a **whole-body HTTP** error (§4.3): the path defers to the **one shared projection** (`json::http_error`, bl-5fe6) that every protocol calls — `kind` from the authoritative status, and the **WHOLE raw body** (envelope, `request_id` and all — not just the inner `error` object) rides `provider_detail` verbatim so nothing is discarded. `message` is best-effort (`error.message` here, but a bare string / `detail` / the raw body for other dialects). `request_id` is present on HTTP responses, absent in SSE `error` events.
- `error.type` lands in `provider_detail` either way; for the **HTTP** case it is informational only — the HTTP **status** is authoritative for the exit (§4.3) — while for the **mid-stream** case it is the signal `decode` reads to choose `kind` (§4.2).

It is emitted as **`Event::Error(..)`** — its own event, **never** folded into `Finish` (architecture.md §3.2). `decode` does not emit `End` (§3.8).

### 4.2 Mid-stream `error` event (after HTTP 200) — exit by `kind`, status NOT consulted (CR-10, resolved in architecture)

```
event: error
data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}
```

This arrives **after** a successful HTTP 200 handshake; the stream then closes with **no** `message_stop`. The 200 handshake status is **NOT consulted** for the exit — architecture.md §8 (CR-10) specifies that an in-band `Event::Error` produced by `decode` carries no governing HTTP status, so its exit is computed from its `CanonicalError.kind` via `ExitClass::from_kind` **directly**. `decode` therefore maps the provider error to a `kind` from `error.type`:

> **Mid-stream `error.type` → `ErrorKind` (last-error-wins).** `decode` maps each
> known Anthropic `error.type` to the `ErrorKind` it would carry as an HTTP error,
> so the in-band and HTTP paths agree on exit and `retryable()` for the same class
> of failure (the `error.type` string is the only signal available — there is no
> governing status here, §4.0):
>
> | `error.type` | `ErrorKind` | exit | `retryable()` |
> |---|---|---|---|
> | `authentication_error`, `permission_error` | `Auth` | **77** | no |
> | `invalid_request_error` | `Provider{400}` | 69 | no |
> | `billing_error` | `Provider{402}` | 69 | no |
> | `not_found_error` | `Provider{404}` | 69 | no |
> | `request_too_large` | `Provider{413}` | 69 | no |
> | `rate_limit_error` | `Provider{429}` | 69 | **yes** |
> | `api_error` | `Provider{500}` | **70** | **yes** |
> | `timeout_error` | `Provider{504}` | **70** | **yes** |
> | `overloaded_error` | `Provider{529}` | **70** | **yes** |
> | *anything else* | `Transport` | 69 | **yes** (safe default) |
>
> An unknown `error.type` falls through to `Transport` (retryable, exit 69) — the no-panic default. The exit codes are the §8 classes throughout (4xx incl. 429 → 69, 5xx → 70, 401/403 → 77). The only behavioral nuance vs. a flat "otherwise → Transport" is that a *known* non-retryable 4xx (`invalid_request_error`, `billing_error`, `not_found_error`, `request_too_large`) maps to `Provider{4xx}` (not retryable) rather than `Transport` (retryable), so `retryable()` does not over-promise a retry on a request the provider rejected on its merits.
>
> `error.message`/`error.type` ride `provider_detail` verbatim so no diagnostic is lost. `decode` emits `[Error(CanonicalError{kind, message, provider_detail:Some})]`; the single terminal `End` is `run`-appended at body EOF (§3.8). Because a decoded `Event::Error` is a clean terminal, the §5.6 premature-EOF injection is suppressed (architecture.md §5.6, CR-9). **Never folded into `Finish`** (architecture.md §3.2). **Last-error-wins**: a later in-band error overrides an earlier exit, and a signal still supersedes everything.

### 4.3 HTTP status → `ErrorKind` → exit code (HTTP errors, status-driven)

For a genuine **non-2xx HTTP** error (§4.0), the status is carried on `frame.status` and `decode` computes `kind = ErrorKind::from_http_status(status)` — `401|403 → Auth`, every other code → `Provider{status}` (which already carries exit + `retryable`). `error.type` is informational only and rides `provider_detail`; it is **not** read for the kind on the HTTP path (only the mid-stream §4.2 path, which has no status, reads it). **The kind comes from the status *before* and *regardless of* whether the body parses:** a non-2xx with a non-JSON body (a proxy's HTML, an empty 5xx) still yields `Provider{status}`, not `Transport` — the carried status is authoritative and is never dropped on a parse failure. **The RAW body is never discarded (bl-5fe6):** `provider_detail` carries the whole parsed body verbatim (not just the inner `error` object), a non-JSON body rides as a `Value::String`, and only an empty body degrades to `provider_detail: None`; `message` falls back through known fields to the body itself, so it is never empty when a body exists. The table below is exactly the shared `from_http_status`:

| HTTP | `error.type` | `ErrorKind` | exit (architecture.md §8) |
|---|---|---|---|
| 400 | `invalid_request_error` | `Provider{status:400}` | **69** |
| 401 | `authentication_error` | `Auth` | 77 |
| 402 | `billing_error` | `Provider{status:402}` | 69 |
| 403 | `permission_error` | `Auth` | 77 |
| 404 | `not_found_error` | `Provider{status:404}` | 69 |
| 413 | `request_too_large` | `Provider{status:413}` | 69 |
| 429 | `rate_limit_error` | `Provider{status:429}` (retryable computed `true`) | 69 |
| 500 | `api_error` | `Provider{status:500}` | 70 |
| 504 | `timeout_error` | `Provider{status:504}` | 70 |
| 529 | `overloaded_error` | `Provider{status:529}` | 70 |

**Exit rule (from architecture.md §8, restated):** provider 4xx (incl. 400, 429) → **69**; 5xx (incl. 500/504/529) → **70**; 401/403 → **77**. A **provider-returned 400 is `Provider{status:400}` → 69** (architecture.md §8, "`Provider` HTTP 4xx") — it is an upstream client error, distinct from adapter-side **malformed-stdin** input, which is the `ParseInput` → 64 case (architecture.md §8, "malformed stdin JSON"). **The sibling OpenAI chat mapping (openai-chat-mapping.md) §4.2 maps a provider 400 identically** (`Provider{400}` → 69), so a caller scripting on exit codes gets the same code from both adapters for the same class of upstream rejection (§6 cross-spec note). The `error.type` string lands in `provider_detail`; the HTTP **status** (not the string) decides the exit.

`retryable()` is **computed** (`Transport`, or `Provider{status}` with `status==429 || status>=500`) — never stored (architecture.md §3.3). So `429`/`500`/`504`/`529` and a post-200 mid-stream `Transport`/`Provider{>=500}`/`Provider{429}` error are retryable; `400`/`401`/`403`/`404`/`413` are not.

---

## 5. Golden FIXTURES this protocol contributes

Recorded from real streams, committed verbatim under `tests/fixtures/` (architecture.md §9.2). Each is decoded deterministically under adversarial rechunking incl. `MidUtf8` / `MidJsonNumber` (architecture.md §9.3). This protocol contributes:

| Fixture | Captures | Decodes to (shape) |
|---|---|---|
| `anthropic_messages_basic` | basic text stream — the assistant replying `Hello` (chunked `"Hel"`+`"lo"`), `stop_reason:"end_turn"` | `MessageStart, Usage, ContentStart{0,Text}, ContentDelta{0,TextDelta}*, ContentStop{0}, Usage, Finish{Stop}` (`End` `run`-appended; `message_stop` sets `terminated`). The §3.9 trace; **this protocol's half of the cross-check** (§5.1). |
| `anthropic_messages_tools` | text block (idx 0) then `tool_use` (idx 1) with `input_json_delta` fragments (first is `""`) | `ContentStart{1,ToolUse{id,name}}` emitted **before** any `JsonDelta` (native identity-first); each block closed by its `content_block_stop`; `Finish{ToolUse}` |
| `anthropic_messages_thinking_tools` | thinking block w/ `signature` (carries a `signature_delta`), then tool/text | `ContentStart{Thinking{id:None}}` → `ThinkingDelta*` → `ContentDelta{SignatureDelta}` (bl-61a9: the signature now SURFACES just before the stop) → `ContentStop`; a sink folds the `SignatureDelta` onto `Content::Thinking.signature` |
| `anthropic_messages_refusal` | **HTTP 200**, `stop_reason:"refusal"` + `stop_details`, empty content | `Finish{Refusal{category,explanation}}`, **exit 0** — NOT an `Error`. (Recorded from a live **streamed** refusal to settle CR-8 / §3.7.) |
| `anthropic_messages_pause` | server-tool loop hits its iteration limit → `stop_reason:"pause_turn"` | `ContentStart{ServerToolUse{id,name}}` → `JsonDelta` → `ContentStop` (the block now SURFACES — CR-4 resolved), then `Finish{Pause}` (§3.10) |
| `anthropic_messages_web_search` | web-search round: text, `server_tool_use` (input as `input_json_delta` fragments), `web_search_tool_result` with the **full content array inline at start and NO delta**, final text, `end_turn` | `ContentStart{ServerToolUse}` + `JsonDelta*` + `ContentStop`; `ContentStart{ServerToolResult{kind:"web_search_tool_result",…,content}}` then immediately `ContentStop` (proves the at-START contract); `Finish{Stop}` |
| `anthropic_messages_nonstream_web_search` | the same web-search round as ONE `stream:false` body | `decode_full` explode→replay: `server_tool_use` through the shared `input_json_delta` synthetic-delta arm; the result block opens on its inline `content`, no delta (§3.4) |
| `anthropic_error_overloaded` | **HTTP 529** non-2xx body (`overloaded_error`, whole-body frame §4.0) | `Error(CanonicalError{Provider{529}, "Overloaded", provider_detail})`, **exit 70**, no `End` from `decode` (`run`-appends it) |

(The OpenAI side contributes `openai_chat_basic`, `openai_chat_tools`, `openai_error_4xx`/`401`, plus `openai_chat_usage`/`openai_chat_refusal_*`/`openai_error_5xx`/`openai_chat_other_finish` per the OpenAI chat mapping (openai-chat-mapping.md) §5.)

Universal invariants checked over **every** Anthropic fixture (architecture.md §9.2): decode + the `run`-appended terminator ends in exactly **one** `End`; `decode` itself emits **zero** `End`; every `ContentDelta.index` has a preceding `ContentStart` and a following `ContentStop` (on the normal/pause paths; truncated streams are scoped out per §3.10); `Usage` fields are `Option`.

### 5.1 This protocol's half of the cross-check (the single-source-of-truth proof)

architecture.md §3.6 / §9.2 require: `anthropic_messages_basic.sse` and `openai_chat_basic.sse` represent the **same logical response** and a property test asserts:

```
normalize(decode_all(anthropic)) == normalize(decode_all(openai))
```

where `normalize` drops only provider-inherent identity and `Usage` events (the one convention pinned on both sides).

**Anthropic's half** is fixed precisely so the equality is writable: `anthropic_messages_basic` is the assistant replying with the literal text **`Hello`** (chunked `"Hel"` + `"lo"`), `stop_reason:"end_turn"`, with `message_start.usage` of `input_tokens:12, output_tokens:1` and a final `message_delta.usage` of `output_tokens:2`. Its decoded `Vec<Event>` (the §3.9 trace) reduces under `normalize` to:

```
MessageStart{role:Assistant}              // id/model dropped: provider-inherent identity
ContentStart{0, Text}
ContentDelta{0, TextDelta("Hel")}
ContentDelta{0, TextDelta("lo")}
ContentStop{0}
Finish{Stop}
End
```

**`normalize` is a single deterministic reduction**, defined identically on both sides:

1. **Drop `MessageStart.id` and `.model`** (provider-specific identifiers — presence/shape compared by the reduced `MessageStart{role:Assistant}`, never the literal strings).
2. **Drop every `Usage` event entirely** (both the `message_start` `Usage` and the `message_delta` `Usage` on the Anthropic side disappear; the OpenAI `*_basic` side emits none because it omits `include_usage`). There is **no** residual claim about `Usage` field presence — `Usage` is removed wholesale, so the load-bearing `cache_read_tokens:Some(0)`-vs-`None` distinction (architecture.md §3.2) is **never forced through the equality.** The usage path is exercised by the dedicated usage fixtures, not by the cross-check.
3. Nothing else is dropped.

The reduced vector above is **byte-identical** to the OpenAI half (the OpenAI chat mapping (openai-chat-mapping.md) §5.1). The `(ContentStart, ContentDelta*, ContentStop)` triple is identical downstream whether native here or synthesized on OpenAI (architecture.md §3.2); the `MessageStart → text triple → Finish{Stop} → End` skeleton matches exactly. The matching `openai_chat_basic` (the OpenAI chat mapping (openai-chat-mapping.md) §5) encodes the same logical "Hello" reply, so this equality holds — that pairing is the executable proof that the canonical model is one model, not two (architecture.md §3.6).

**Provider-inherent differences excluded from the equality (documented so no future pairing assumes equality):**

- **`Usage` presence/values** — Anthropic emits `Usage` natively (twice); OpenAI only with `include_usage`. Excluded by dropping all `Usage` events on both sides (rule 2 above). A `Usage` cross-check is **not** writable as strict equality and is not attempted.
- **`FinishReason::StopSequence` vs `Stop`** — a stop-sequence finish decodes to `StopSequence` here but `Stop` on OpenAI (§3.5; the OpenAI chat mapping (openai-chat-mapping.md) §3.5). The basic pairing uses `end_turn`→`Stop`, so it is not hit; a stop-sequence pairing is **excluded** from the equality.
- **The early post-`MessageStart` `Usage`** — Anthropic-only; subsumed by rule 2.

---

## 6. Edge cases & change requests

Every item below is a place the Anthropic wire and the canonical model do not align. Each is either resolved within this spec (no CR), **resolved in architecture.md** (the canonical type/contract was revised to express the fact), or still **deferred** as a genuine open gap. Per the derivation rule, none is silently deviated.

### Resolved here (no canonical change)

- **`stop` → `stop_sequences` rename** (§2.2): a pure field-name projection; canonical `stop` is unchanged.
- **`Role::Tool` → `"user"` + `tool_result`** (§2.3): the adapter owns this projection; the core never branches on it (architecture.md §3.1). No change.
- **`system` hoisting** (§2.4): `Role::System` content / `req.system` → top-level `system`. The canonical model already separates `req.system` from `messages`; this is just where the wire puts it. No change.
- **`tool_choice` spellings** (§2.7): the four canonical intents map cleanly to the four wire shapes. No change.
- **`disable_parallel_tool_use` ← `parallel_tool_calls`** (§2.7): the one canonical *addition* — a lifted known knob (architecture.md §3.1) that both providers spell differently. It nests in `tool_choice`, so it cannot ride `extra` (top-level only); the adapter folds `Some(false)` into the `tool_choice` object.
- **Single-Text collapse to bare string** (§2.3/§2.4): wire-equivalent; always safe to emit the array. No change.
- **`extra` top-level merge, typed fields win** (§2.1.1/§2.8): the severability valve of architecture.md §2/§4.1, with the same `extra` precedence as the OpenAI chat mapping (openai-chat-mapping.md) §2.1.1. No change.
- **`signature_delta` → `Delta::SignatureDelta`** (§3.4, bl-61a9): CR-5 resolved — the signature now surfaces as a `ContentDelta` in wire-arrival order and a sink folds it onto `Content::Thinking.signature`. See "Resolved in architecture.md (continued) — bl-61a9" below.
- **`content_block_start` text seed** (§3.4): the wire's start `text` is always `""`, so there is **no** seed-delta branch (an earlier draft had one — removed as unreachable; architecture.md §9.5: an unhittable branch is reframed away, not kept-but-uncovered). No change.
- **Open blocks at terminal events** (§3.10): the normal/pause wire always closes every block before the terminal `message_delta`, so the `ContentStop` invariant holds without synthesis; only truncated streams leave a block open, and those are scoped out. No change.
- **Provider HTTP 400 → `Provider{400}` → 69** (§4.3): matches architecture.md §8 (provider 4xx → 69) and the sibling OpenAI chat mapping (openai-chat-mapping.md) §4.2 exactly. No change — recorded as the cross-spec agreement.

### Resolved in architecture.md (the canonical model was revised — no longer open CRs)

- **`redacted_thinking` → `Content::RedactedThinking{data}`** (§2.5/§3.4). architecture.md §3.1 added `Content::RedactedThinking { data: String }` (and §3.2 the mirroring `ContentKind::RedactedThinking { data }` — the streamed kind now carries the `data` INLINE at start too, bl-61a9), opaque and round-tripped verbatim. The Anthropic mapping now maps `redacted_thinking` wire blocks ⇄ `Content::RedactedThinking` cleanly, on BOTH the request side and the decoded event stream. **The prior lossy interim hack — folding `redacted_thinking` into `Thinking` via the signature slot — is dropped.** The OpenAI mapping never produces this variant (the empty-set rule).
- **Non-text-slot rejection for `req.system` / `ToolResult.content`** (§2.4/§2.5). architecture.md §3.1 keeps both slots permissive `Vec<Content>` canonically (single source of truth) and specifies that an adapter targeting a **text-only wire slot** that receives non-`Text` content **rejects at `encode`** with `ErrorKind::ParseInput` (exit 64) — a documented runtime degradation, not a type change. The Anthropic `system` slot (text-only) and `tool_result.content` slot (text/image-only) implement exactly this. Applied uniformly with the OpenAI adapter's text-only `system`/`developer`/`tool` slots.
- **Wire serde shapes are externally-tagged** (§3.4/§3.9). architecture.md §3.2 dropped `serde(tag=…)` from `Content`, `ContentKind`, and `Delta`. `Content` uses a custom string-or-object representation (a bare wire string ⇄ `Content::Text(String)`; an object decodes by its `type`). `ContentKind` is external-tagged with struct-like empty unit variants rendering `"kind":{"text":{}}` etc.; `Delta` is external-tagged with newtype variants rendering `"delta":{"text_delta":"Hel"}` etc. `Event` KEEPS `serde(tag="type")` (its outer envelope), and `Event::Raw` is `serde(skip)` (never an NDJSON line; raw mode writes bytes verbatim). The cited byte samples in §3.4/§3.9 reflect this shape.
- **Premature-EOF vs clean terminal — `DecodeState.terminated`** (§3.8, formerly CR-9). architecture.md §5.6 now carries `DecodeState.terminated: bool`; `decode` sets it `true` on consuming `message_stop`, and `run` injects the premature-EOF `Error{Transport}` + exit 69 **only if `!terminated`**. `decode` still NEVER emits `End` — `run` owns the single `End`. A decoded `Event::Error` is also a clean terminal. Shared with the OpenAI mapping (`[DONE]` sets `terminated`).
- **Post-200 mid-stream error exit by `kind`** (§4.2, formerly CR-10). architecture.md §8 now specifies that an in-band mid-stream `Event::Error` drives the exit from its `kind` via `from_kind` **directly** — the 2xx handshake status is NOT consulted. The Anthropic mapping's `decode` maps each known mid-stream `error.type` to the `kind` it would carry as an HTTP error (the full table in §4.2: auth/permission → `Auth`/77; the 4xx types → `Provider{4xx}`/69; 5xx-class → `Provider{>=500}`/70; unknown → `Transport`/69 default), last-error-wins.

### Cross-spec note (not a change — a consistency the pairing relies on)

- **`extra` precedence, the 400 mapping, terminator/`terminated` ownership, and the cross-check `normalize`/usage convention are pinned identically in the OpenAI and Anthropic mappings.** Specifically: typed fields win over `extra` (§2.1.1 = openai-chat-mapping.md §2.1.1); provider 400 → `Provider{400}` → 69 (§4.3 = openai-chat-mapping.md §4.2); `decode` never emits `End` and sets `terminated` on the terminal marker (§3.8 = openai-chat-mapping.md §3.6); both `*_basic` fixtures omit/normalize-out `Usage` so the cross-check is over the text skeleton only (§5.1 = openai-chat-mapping.md §5.1); `StopSequence`-vs-`Stop` is provider-inherent and excluded (§5.1 = openai-chat-mapping.md §5.1). These are not architecture changes — they are the consistency that makes the §5.1 equality test writable.

### Resolved in architecture.md (continued) — CR-4, server-tool blocks

- **CR-4 — server-tool BLOCKS are RESOLVED as opaque verbatim passthrough (architecture.md §3.1/§3.2); only the usage COUNTER stays deferred.** `Content::ServerToolUse{id,name,input}` / `Content::ServerToolResult{kind,tool_use_id,content}` (§2.5) round-trip request-side blocks verbatim; `ContentKind::ServerToolUse{id,name}` / `ContentKind::ServerToolResult{kind,tool_use_id,content}` (§3.4) surface them on decode. The result wire tag is an **open set carried as data**: decode keys on the tag SUFFIX (`ends_with("_tool_result")` && ≠ `tool_result`), encode re-emits `kind` verbatim, so the whole `*_tool_result` family round-trips with zero per-tool knowledge. Enablement is typed via `Tool::Provider{kind,name,config}` (§2.6). These are **additive `v=1` kinds — NO `EVENT_SCHEMA_VERSION` bump** (the empty-set/`v=1` rule the architecture predicted held: unmapped kinds already rode `ContentKind::Other`). Betas are the **caller's job** via row `beta_headers` — brazen bakes in no tool-version→beta tables. brazen still executes nothing: TRANSLATE (project declarations), CLASSIFY (server results are done, never pending), ROUND-TRIP (verbatim, or replay 400s). Web-search fixtures are **unblocked** (`anthropic_messages_web_search` / `anthropic_messages_nonstream_web_search`, §5). **Still deferred (the residual):** `usage.server_tool_use.web_search_requests` has no canonical `Usage` field — it rides `provider_detail` (§3.6); and no `WebSearch`/citation NORMALIZATION exists (passthrough only).

### Resolved in architecture.md (continued) — bl-61a9, reasoning round-trip

The owner ruling (2026-07-08) — "reasoning replay was probably appropriately punted from 0.0.1, it's time now" — **formally supersedes the "low urgency" assessments of CR-2 and CR-5**: reasoning replay IS the agentic tool loop a `--json` harness runs, so both are now resolved.

- **CR-2 — `Thinking{signature: None}` drop-on-encode is the documented, CORRECT rule (RESOLVED, no type change).** Anthropic 400s on a thinking block whose signature is absent on multi-turn replay; `encode` therefore drops a signature-less `Thinking` (§2.5, `blocks.rs`) rather than send a block that would 400. This is exactly the empty-set/`None` semantics of architecture.md §3.1 — the rule stays. The decode side now CAPTURES the signature (`Delta::SignatureDelta`, below) so a round-tripping harness has a signature to replay, which is what closes the loop CR-2 flagged.
- **CR-5 — `signature_delta` → `Delta::SignatureDelta(String)` (RESOLVED, bl-61a9).** architecture.md §3.2 added `Delta::SignatureDelta(String)`; the Anthropic `signature_delta` now emits `ContentDelta{index, SignatureDelta}` in wire-arrival order (just before the thinking block's `content_block_stop`), and a sink folds it onto `Content::Thinking.signature`. Google's `functionCall` `thoughtSignature` rides the SAME delta (folding to `ToolUse.signature`), so the one variant serves both dialects — `ContentKind::ToolUse` is unchanged. Additive under the `v=1` contract (new `Delta` variant, `Delta::Other` catches it on a pinned consumer) — **no `EVENT_SCHEMA_VERSION` bump**. `decode_full` (non-stream) reconstructs the same `SignatureDelta` from the `content[i]` thinking block's `signature` field, driven through the shared `content_block_delta` handler.

### Still deferred (genuine open gaps — not yet resolved in architecture.md)

- **CR-6 — `StopSequence` drops the matched string.** When `stop_reason=="stop_sequence"`, the wire reports the matched sequence in top-level `stop_sequence` / `delta.stop_sequence`. Canonical `FinishReason::StopSequence` carries **no payload**, so the matched string is dropped. *Requested change (if the matched sequence must be preserved):* `StopSequence { matched: String }`. v0.1 drops it. **Low urgency.**

- **CR-7 — `model_context_window_exceeded` has no dedicated `FinishReason`.** Mapped to `Other("model_context_window_exceeded")`. Semantically distinct from `Length` (max_tokens). `Other(String)` correctly avoids conflation and never panics (architecture.md §9.5), so **no change is strictly required** — raised only if callers need to branch on it as a first-class reason.

- **CR-8 — streamed refusal `stop_details` is unverified.** §3.7: whether `stop_details{category, explanation}` rides the streamed `message_delta` is **not documented**. *Action (not a type change):* the `anthropic_messages_refusal` fixture (§5) MUST be recorded from a **live streamed refusal** to settle whether `category`/`explanation` are available in-stream. If absent, decode emits `Finish{Refusal{category:"", explanation:None}}` (still exit 0, still never an `Error`). **This blocks finalizing the refusal fixture, not the canonical model.**

---

## 7. Summary of decisions (this spec is decisive)

- **Framing:** `Sse`. Decode against `data.type`, not the `event:` name. A non-2xx error body arrives as a whole-body frame via the SSE-decoder contract (§4.0).
- **Request:** `system` hoisted top-level; `Role::Tool` → `"user"`+`tool_result`; `stop` → `stop_sequences`; `Thinking`/`RedactedThinking` first in assistant turn with `signature`/`data` verbatim; `max_tokens` required (config-resolution guarantees `Some`); `parallel_tool_calls: Some(false)` folds to `tool_choice.disable_parallel_tool_use` (§2.7, nested — not `extra`); `extra` merged top-level with typed fields winning (§2.1.1, same as the OpenAI mapping); auth headers set by `Auth`; `anthropic-version` from `ctx.beta_headers`. Text-only wire slots (`system`, `tool_result.content`) reject non-`Text` content with `ParseInput`/64 (architecture.md §3.1).
- **Response:** native `content_block_start` satisfies identity-before-content; `content_block_start` text is always `""` (no seed-delta branch); `redacted_thinking` opens `ContentStart{RedactedThinking{data}}` carrying the opaque blob INLINE at start (bl-61a9); `input_json_delta` → `JsonDelta` emitted DIRECTLY, never buffered or parsed mid-stream; `signature_delta` → `Delta::SignatureDelta` in wire-arrival order, folded onto `Content::Thinking.signature` by a sink (bl-61a9); `Usage` cumulative, `Option`, last-wins per field; `message_delta` emits `Finish` only when `stop_reason` non-null. `ContentKind`/`Delta` render externally-tagged (architecture.md §3.2).
- **Refusal:** `Finish{Refusal}`, HTTP 200, exit 0 — never an `Error`.
- **Terminator:** `decode` never emits `End`; `message_stop` emits `[]` and sets `DecodeState.terminated`; mid-stream `error` emits `[Error]`; `run` appends the single `End` at body EOF — exactly one `End` in all cases, identical to the OpenAI mapping. The §5.6 premature-EOF injection fires only when `!terminated` (architecture.md §5.6).
- **Errors:** HTTP status drives the exit for a failed handshake (provider 4xx→69 incl. 400/429, 5xx→70, 401/403→77 — same as the OpenAI mapping §4.2); a post-200 mid-stream `error` is exited by its decoded `kind` via `from_kind`, status NOT consulted (`error.type`→`kind` per the §4.2 table: auth/permission→`Auth`/77, 4xx types→`Provider{4xx}`/69, 5xx-class→`Provider{>=500}`/70, unknown→`Transport`/69), last-error-wins (architecture.md §8); `error.type` informs `provider_detail`; `retryable()` computed.
- **Server tools (CR-4): resolved as opaque verbatim passthrough.** `server_tool_use` ⇄ `Content::ServerToolUse`/`ContentKind::ServerToolUse`; the `*_tool_result` family ⇄ `ServerToolResult{kind,…}` with the wire tag carried as data (suffix-matched at decode, re-emitted verbatim at encode — the open-set rule); enablement via `Tool::Provider{kind,name,config}` (`Tool::Custom` stays the custom shape). Additive `v=1` kinds — no version bump; betas ride the caller's row `beta_headers`. The `usage.server_tool_use.*` counter is the one residual (rides `provider_detail`).
- **CRs:** resolved-in-architecture — `redacted_thinking` → `Content::RedactedThinking{data}` (now carried on the stream too, bl-61a9), non-text-slot rejection, external-tagged serde, `terminated`/premature-EOF, post-200 mid-stream exit-by-`kind`, CR-4 server-tool blocks (open-set suffix round-trip; the usage counter the residual), and **CR-2/CR-5 reasoning round-trip (bl-61a9): `signature_delta` → `Delta::SignatureDelta` folded onto `Thinking.signature`, the drop-on-`None`-encode rule kept**. Still deferred — CR-6/7/8 documented for visibility (StopSequence payload, model_context_window_exceeded, the live-refusal fixture capture). None silently deviated.

CITATIONS: https://platform.claude.com/docs/en/api/messages · https://platform.claude.com/docs/en/build-with-claude/streaming · https://platform.claude.com/docs/en/api/errors · https://platform.claude.com/docs/en/build-with-claude/extended-thinking · https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons · https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback · https://platform.claude.com/docs/en/build-with-claude/vision