# Provider rows — Mistral, OpenAI responses, Google generative-ai, Ollama

> **Living document.** Edited like code. This spec is a set of **lossy projections** onto and back from the canonical model of architecture.md; it MUST NOT contradict it. Where a wire dialect cannot express a canonical fact (or vice-versa), this spec raises a **change request to architecture.md** (§7) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — especially §3 (the canonical model, the single source of truth each dialect projects onto/from), §3.4 (the native-terminator→`End` table), §4.1 (the `Protocol` trait, `ProviderCtx`, `HeaderSpec`, `Framing`), §4.2 (Provider is DATA — the embedded TOML rows), §4.4 (dispatch with no match-on-provider), §4.6 (the severability proof), §11 (a new protocol = one module).
> **Sibling mapping specs (referenced, not duplicated):** [OpenAI chat](openai-chat-mapping.md) · [Anthropic messages](anthropic-messages.md). NDJSON framing and `Frame`/`DecodeState` mechanics live in the SSE-decoder spec (planned); this spec cites the framing **contract** (architecture.md §3.4/§4.1) and never redefines the framer.

---

## 1. Purpose & Scope

This spec defines the **data-and-dialect additions beyond the v0.1 slice** (Anthropic messages + OpenAI chat). Four providers, graded against the severability rubric of architecture.md §4.6, in ascending cost:

| Provider | Cost | What it adds |
|---|---|---|
| **Mistral** | **one `[[provider]]` row, ZERO Rust** | a row on `protocol = "openai_chat"` + `auth = "bearer"`. The severability proof. |
| **OpenAI responses** | one module + one `ProtocolId` arm + one `Registry::builtin()` insert (+ a row) | `mod openai_responses`; `response.completed` → `Event::End`. SSE framing. |
| **Google generative-ai** | one module + one arm + one insert (+ a row) | `mod google_genai`; the `x-goog-api-key` `HeaderSpec` as pure **row data** (no new `Auth`); last `finishReason`-bearing chunk → `Event::End`. SSE framing. |
| **Ollama** | one module + one arm + one insert (+ a row) | `mod ollama_chat`; **NDJSON** framing (`framing() -> Ndjson`); `{"done":true}` → `Event::End`. |

The thesis of architecture.md §4 — **a provider is a row of data; a protocol/auth is a trait impl keyed by an enum id; the pipeline dispatches through a registry lookup, never a `match` on a vendor name** — is exactly what this spec exercises. Each new dialect derives its mapping table from the canonical model of architecture.md §3 (the single source of truth), and each new terminator normalizes to the **same single `Event::End`** of architecture.md §3.4.

### 1.1 Inherited invariants (the grading rubric every row here upholds)

Restated from architecture.md §3–§5 so this spec is self-contained; identical to the invariants the sibling mappings uphold (openai-chat-mapping.md §1.1, anthropic-messages.md §1.1):

1. **`Protocol` is PURE and object-safe** — `encode`/`decode`/`framing` touch no IO, no clock, no creds; cross-frame state lives in the caller-owned `&mut DecodeState`, so each impl is shareable as `&'static dyn Protocol`.
2. **Every impl is vendor-blind** — it reads only `ProviderCtx { base_url, model (alias-resolved), api_header, beta_headers, extra }`; the vendor name was spent on the registry lookup before `encode` runs (architecture.md §4.1).
3. **Auth is not Protocol** — `encode` sets only body + non-auth headers; the auth header is set by `Auth::apply` reading `ctx.api_header` as DATA (architecture.md §4.5).
4. **`content` is ALWAYS `Vec<Content>`**; a bare wire string decodes to `vec![Content::Text(..)]`.
5. **Identity precedes content** — `ContentStart{index, kind}` (carrying tool id/name) is emitted before any `ContentDelta` for that index; an adapter whose wire lacks a block-open **synthesizes** it (architecture.md §3.2, §3.6).
6. **Tool-call arguments stream as `Delta::JsonDelta(String)` fragments** — never parsed mid-stream; parsed to a `Value` only when folding to `Content::ToolUse` (architecture.md §3.6).
7. **Exactly ONE `Event::End` per response.** `decode` **NEVER emits `End`** — the single terminator is the `sink.write(&Event::End)` the `run` loop appends once after the body iterator drains (architecture.md §4.4). Each protocol's terminal marker decodes to `[]` and sets `state.terminated = true` (architecture.md §3.5, CR-9), suppressing the premature-EOF injection (architecture.md §5.6).
8. **Refusal is a `Finish{Refusal}`, never an `Error`** (architecture.md §3.2); HTTP 200, exit 0. `Error` is its own event, never folded into `Finish` (architecture.md §3.3).
9. **`Usage` fields are `Option`** — `None` is "unknown", never a fabricated `0` (architecture.md §3.2).
10. **`decode` is pure over `(frame, &mut DecodeState)`**; provider-error parsing lives in `decode`, the HTTP status is peeked separately for the exit code (architecture.md §8).

The HTTP-status→`ErrorKind`→exit table (architecture.md §8: provider 4xx→69 incl. 429, 5xx→70, 401/403→77, malformed-stdin→64) and the non-2xx whole-body-frame decoder contract (openai-chat-mapping.md §4.0, anthropic-messages.md §4.0) are **shared by every protocol below** — each error section names only its dialect's error-envelope shape and defers the status→exit mapping to that shared table.

---

## 2. Mistral — the severability proof (one row, ZERO Rust)

Mistral's Chat Completions endpoint speaks the OpenAI `chat/completions` dialect verbatim. So the **entire** Mistral diff is one `[[provider]]` row in the embedded `defaults.toml` (architecture.md §4.2):

```toml
[[provider]]
name = "mistral"                                          # table key only — never matched on in the pipeline
base_url = "https://api.mistral.ai/v1"
protocol = "openai_chat"                                  # reuses the OpenAiChat Protocol impl VERBATIM
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
```

### 2.1 What makes this zero-code

`protocol = "openai_chat"` is a **registry key**, not a dispatch branch (architecture.md §4.2, §4.4). At resolution time `cfg.provider.protocol == ProtocolId::OpenAiChat`, so `run` looks up `registry.protocols[&ProtocolId::OpenAiChat]` — the **same `&OpenAiChat` impl** that serves the `openai` row. `encode`/`decode`/`framing` run unchanged; they never learn the provider is Mistral because `ProviderCtx` carries **no name** (architecture.md §4.1). `auth = "bearer"` likewise reuses `&BearerAuth`, which reads the header to set (`Authorization: Bearer …`) from `ctx.api_header` as DATA (architecture.md §4.5, §7). The request/response/error mapping is **exactly** openai-chat-mapping.md §2–§4 — nothing here re-specifies it.

**The deletion test (architecture.md §4.6).** Delete the four-line row → Mistral is gone, cleanly, with no dangling code, because there was never any Mistral code. A request naming `--provider mistral` then resolves to no row → `Config` error, exit 78 (architecture.md §4.3). No module, no enum arm, no insert touched. This is the lower bound of the rubric: **adding a provider that reuses an existing protocol+auth is pure data.**

### 2.2 Routing & aliases

The row ships **no** `model_aliases` and **no** `default_max_tokens` — Mistral Chat Completions does not require `max_tokens` (so `req.max_tokens` stays `None` and is omitted, openai-chat-mapping.md §2.1), and alias tables are optional shorthand (architecture.md §4.3). A user routes to Mistral by `--provider mistral`, or by adding aliases in their own config file (the file layer is the same `PartialConfig` schema, architecture.md §6.1) — an operator concern, not a built-in. Identity passthrough (`model_aliases.get(model).unwrap_or(model)`, architecture.md §4.3) means an unaliased Mistral wire id (`mistral-large-latest`) passes through verbatim once the provider is named.

### 2.3 Mistral wire deviations from OpenAI chat — and why they need no code

Mistral's dialect is OpenAI-chat with a few narrowings. The decision in each case: **does it fit in row data / `extra`, or does it force code?** All fit in data:

| Mistral deviation | Disposition | Rationale |
|---|---|---|
| **`tool_choice` accepts `"any"`** (alongside `auto`/`none`/named) where OpenAI uses `"required"` for "must call a tool" | passthrough via `extra` if a caller needs the literal `"any"`; the canonical `ToolChoice::Any` already encodes to OpenAI's `"required"` (openai-chat-mapping.md §2.6), which Mistral also accepts | the canonical intent maps to a wire value Mistral honors; the alternate spelling is a long-tail knob, not a code branch |
| **No structured `refusal` output field** (the OpenAI structured-refusal channel of openai-chat-mapping.md §3.5) | none needed — `state.refusal` simply stays empty, so the `Finish` reason is computed from `finish_reason` alone (openai-chat-mapping.md §3.5). The empty set is not a special case (architecture.md §3.1) | the OpenAI `decode` already handles "no `delta.refusal` ever arrives"; a provider that never sends it exercises the empty path |
| **`prompt`/FIM, `safe_prompt`, `random_seed`** and other Mistral-only knobs | `req.extra` passthrough (the long-tail valve, architecture.md §3.1; openai-chat-mapping.md §2.1.1) | no canonical home, forwarded verbatim, typed fields win on a name clash |
| **Stricter JSON-Schema validation on `tools[].function.parameters`** | none — `input_schema` is passed through verbatim (openai-chat-mapping.md §2.5); a rejected schema surfaces as a provider 400 → `Provider{400}` → 69 (architecture.md §8) | validation is the provider's; brazen does not pre-validate the long tail (architecture.md §3.1, the owned cost) |
| **`max_tokens` not deprecated** (no `max_completion_tokens` rename for reasoning models) | none — the row sets no rename signal, so `encode` emits `max_tokens` (openai-chat-mapping.md §2.7); if a future Mistral reasoning model needs the rename it is a **row/resolution** signal, not code (openai-chat-mapping.md §2.7, severability) | key selection is row-data driven, never a vendor branch |

**Conclusion:** every Mistral deviation is absorbed by `extra` passthrough, an empty-set path the OpenAI `decode` already takes, or a row/resolution signal. None reaches `encode`/`decode`/`framing`. Mistral remains **one row, zero Rust** — the severability proof holds.

---

## 3. OpenAI responses — `mod openai_responses` (new dialect)

OpenAI's **Responses API** (`POST {base_url}/responses`, SSE) is a *different wire dialect* from Chat Completions: a typed event stream (`response.*`, `response.output_item.*`, `response.*.delta`) rather than `chat.completion.chunk` deltas. It is the first true "new dialect" cost in the rubric: **one module + one `ProtocolId` arm + one `Registry::builtin()` insert**.

### 3.1 Cost & the provider row

```rust
// registry.rs — ONE insert (architecture.md §4.4)
protocols.insert(ProtocolId::OpenAiResponses, &OpenAiResponses);
```
```rust
// config/provider.rs — ONE enum arm
enum ProtocolId { OpenAiChat, AnthropicMessages, OpenAiResponses, /* GoogleGenAi, OllamaChat */ }
```
```toml
# defaults.toml — a row (data) selecting the new protocol
[[provider]]
name = "openai-responses"
base_url = "https://api.openai.com/v1"
protocol = "openai_responses"
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
```

`framing(&self) -> Framing { Framing::Sse }`. The shared `SseDecoder` (SSE-decoder spec) hands `decode` one parsed `Frame` per `data:` payload; this dialect's frames carry a JSON object discriminated by its `"type"` field (e.g. `response.output_text.delta`). The `event:` SSE name mirrors `data.type`; **decode against `data.type`** (as Anthropic does, anthropic-messages.md §3).

### 3.2 REQUEST mapping — canonical → `responses` wire

`encode(req, ctx)` builds the `POST {ctx.base_url}/responses` body. Auth header set by `BearerAuth` (architecture.md §4.5). The Responses API folds system + messages into a single `input` array and renames a few fields:

| Canonical (`CanonicalRequest`) | Responses wire field | Rule |
|---|---|---|
| `model` | `"model"` | `ctx.model` (alias-resolved). Always present. |
| `system: Option<Vec<Content>>` | `"instructions"` (string) | text-only top-level field. `Some(non-empty)` → concatenated text; `None`/`Some(vec![])` → omit. Non-`Text` content → `Error{ParseInput}`/64 (the non-text-slot rejection, architecture.md §3.1; same as openai-chat-mapping.md §2.3). |
| `messages: Vec<Message>` | `"input"` (array of typed items) | §3.3. |
| `tools: Vec<Tool>` | `"tools"` | **omit when empty.** Else `{type:"function", name, description?, parameters}` — **flat** (no nested `function` envelope, unlike Chat Completions). |
| `tool_choice: ToolChoice` | `"tool_choice"` | `Auto`→omit; `Any`→`"required"`; `None`→`"none"`; `Tool{name}`→`{type:"function", name}`. |
| `max_tokens: Option<u32>` | `"max_output_tokens"` | **RENAME.** `Some(n)`→`n`; `None`→omit. The OpenAI row requires none, so normally `None`. |
| `temperature`/`top_p` | `"temperature"`/`"top_p"` | `Some`→value; `None`→omit. |
| `stop: Vec<String>` | — | **no native stop field on Responses;** a non-empty `stop` rides `extra` if the caller supplies the wire key, else omitted. (Watch item, §7 CR-R1 — a documented narrowing, not a silent drop of a typed field that *is* supported.) |
| `stream: bool` | `"stream"` | the bool. Responses streams `Usage` natively on `response.completed` — **no `stream_options` knob needed** (unlike Chat Completions, openai-chat-mapping.md §2.8). |
| `extra` (`#[serde(flatten)]`) | merged top-level | the long-tail valve: `reasoning`, `text`, `previous_response_id`, `store`, `include`, … Typed fields win (architecture.md §3.1; same precedence as openai-chat-mapping.md §2.1.1). |

#### 3.3 `input[]` — per-`Message` projection

Each canonical `Message` becomes one or more typed input items. The Responses API uses explicit item types rather than role+content:

| Canonical | Responses input item |
|---|---|
| `Message{User, [Text]}` | `{type:"message", role:"user", content:[{type:"input_text", text}]}` |
| `Message{User, [Image{Base64}]}` | `{type:"input_image", image_url:"data:{mt};base64,{data}"}` (data-URI, as Chat Completions, openai-chat-mapping.md §2.2) |
| `Message{Assistant, [Text]}` | `{type:"message", role:"assistant", content:[{type:"output_text", text}]}` |
| `Content::ToolUse{id,name,input}` | `{type:"function_call", call_id:id, name, arguments:to_json_string(input)}` — `arguments` a JSON **string**, not an object |
| `Content::ToolResult{tool_use_id, content, is_error}` | `{type:"function_call_output", call_id:tool_use_id, output:<text>}` — text-only slot; non-`Text` content → `Error{ParseInput}`/64. `is_error` surfaced textually (prefix), no native field (same degradation as openai-chat-mapping.md §2.4, §7 CR-R2) |
| `Content::Thinking{text,signature}` | reasoning items round-trip via `extra`/`include` when present, else **dropped** on re-send — Responses reasoning replay rides `previous_response_id`/encrypted reasoning items, out of scope for v0.1 (§7 CR-R3) |
| `Content::RedactedThinking{data}` | dropped — non-OpenAI variant, never produced by this adapter (the empty-set rule, architecture.md §3.1) |

`Role::System` *in `messages`* projects to `{type:"message", role:"system", ...}` (or `developer` per a row signal, mirroring openai-chat-mapping.md §2.3); `req.system` hoists to `instructions` (§3.2). Both kept distinct (architecture.md §3.1, decision 10).

### 3.4 RESPONSE mapping — `responses` SSE → canonical `Vec<Event>`

The Responses stream is a sequence of typed events. `decode` dispatches on `data.type`. Unlike the synthesized-structure dialects (Google/Ollama), the wire carries explicit block structure, so the **canonical index is the wire `output_index`** (Anthropic-style — never synthesized, never `open.len()`); deltas route by it and `output_item.done` closes by it:

| Wire `data.type` | Canonical events | DecodeState action |
|---|---|---|
| `response.created` / `response.in_progress` | `MessageStart{ id: Some(response.id), model: Some(response.model), role: Assistant }` **once** (gated on `state.started`) | `started = true` |
| `response.output_item.added` with `item.type=="message"` | — (a text block opens lazily on first text delta) | record output index |
| `response.content_part.added` (`part.type=="output_text"`) | **synthesize** `ContentStart{index, Text {}}` | assign canonical index, mark open |
| `response.output_text.delta` (`{delta:"Hel"}`) | `ContentDelta{index, TextDelta(delta)}` | — |
| `response.output_item.added` with `item.type=="function_call"` (carries `call_id`+`name`) | **synthesize** `ContentStart{index, ToolUse{ id: call_id, name }}` — **identity before content** (architecture.md §3.2) | map item→canonical index, mark open |
| `response.function_call_arguments.delta` (`{delta:"{\""}`) | `ContentDelta{index, JsonDelta(delta)}` — **never parsed mid-stream** (architecture.md §3.6) | — |
| `response.output_item.done` (the item-level close — one per output item) | `ContentStop{index}` for a tracked block | remove from open. The inner `response.output_text.done` / `response.function_call_arguments.done` are no-ops (the fragment already streamed); closing on the **outermost** `.done` alone closes each block exactly once |
| `response.reasoning_summary_text.delta` | `ContentDelta{index, ThinkingDelta(delta)}` (block opened with `ContentStart{Thinking {}}` on the matching `reasoning` item add) | — |
| `response.completed` | `Usage`(from `response.usage`, §3.5) then `Finish{reason}` (§3.6); then `[]` and **`state.terminated = true`** | drain any still-open blocks to `ContentStop` first |
| `response.incomplete` | `Finish{Length}` if `incomplete_details.reason=="max_output_tokens"`, else `Finish{Other(reason)}`; sets `terminated` | drain open blocks |
| `response.error` / `response.failed` | `Error(CanonicalError{..})` (§3.7); no `End` | mid-stream error, terminal |
| `response.refusal.delta` / refusal item | accumulate; surfaced as `Finish{Refusal{category:"refusal", explanation:Some(acc)}}` at completion (HTTP 200, exit 0 — architecture.md §3.2) | append to `state.refusal` |

**The terminator (architecture.md §3.4).** `response.completed` (the native terminator) decodes its `Usage`+`Finish` and then sets `state.terminated = true`; `decode` **never emits `End`** — `run` appends the single `End` at body EOF (architecture.md §4.4). Because `terminated` is set, the premature-EOF injection is suppressed (architecture.md §5.6, CR-9). Identical End-ownership discipline to openai-chat-mapping.md §3.6 and anthropic-messages.md §3.8.

### 3.5 `Usage` mapping

`response.usage` on the completion event: `input_tokens`→`input`, `output_tokens`→`output`, `input_tokens_details.cached_tokens`→`cache_read` (`Some` iff present, else `None` — never `0`, architecture.md §3.2), no cache-write equivalent → `cache_write: None`. `total_tokens`/`output_tokens_details.reasoning_tokens` are derivable/long-tail → dropped (reasoning-token detail rides `provider_detail` only if a future need arises). Emitted **before** `Finish` (both ride the one `response.completed` frame; order within the returned `Vec` is `… ContentStop* → Usage → Finish`).

### 3.6 `FinishReason` mapping

`response.completed` carries the terminal status. With `state.refusal` non-empty → `Refusal{category:"refusal", explanation:Some(state.refusal)}` (takes precedence, as openai-chat-mapping.md §3.5). Else by `response.status` / `incomplete_details.reason`:

| condition | `FinishReason` |
|---|---|
| status `completed`, output ended normally | `Stop` |
| any `function_call` item present in output | `ToolUse` |
| `response.incomplete`, reason `max_output_tokens` | `Length` |
| `response.incomplete`, other reason `r` | `Other(r)` |
| `content_filter`-class refusal with no `refusal` text | `Refusal{category:"content_filter", explanation:None}` |
| any unknown status `s` | `Other(s)` — never panics (architecture.md §9.5) |

`StopSequence` is **not produced** (Responses, like Chat Completions, reports a stop-sequence hit as a normal stop — excluded from any cross-check, as openai-chat-mapping.md §3.5). `Pause` is Anthropic-only.

### 3.7 ERROR mapping

A non-2xx handshake arrives as a whole-body frame via the shared decoder contract (openai-chat-mapping.md §4.0); the body is OpenAI's `{"error":{message,type,param,code}}` envelope (same shape as openai-chat-mapping.md §4.1). `decode` emits `Event::Error{kind, message: error.message, provider_detail: Some(error)}`; the HTTP status drives kind+exit per the shared table (architecture.md §8 — 400→`Provider{400}`/69, 401/403→`Auth`/77, 429→`Provider{429}`/69, 5xx→70). A mid-stream `response.error`/`response.failed` (after HTTP 200) is exited by its decoded `kind` via `from_kind`, status NOT consulted (architecture.md §8, CR-10): a 5xx-class `error.type` → `Provider{>=500}`/70, rate-limit → `Provider{429}`/69, otherwise `Transport`/69. Never folded into `Finish` (architecture.md §3.3).

### 3.8 What's NOT touched (severability)

`run`, `resolve`, `parse`, the `Sink`, the canonical model (§3 types), and the `OpenAiChat`/`AnthropicMessages` impls are **unchanged**. `response.completed` normalizes to the same `Event::End` as `[DONE]` and `message_stop`. Delete `mod openai_responses` + the `ProtocolId::OpenAiResponses` arm + the one insert → gone; rows naming `openai_responses` then fail at resolve with a `Config` error (78). This is exactly the rubric's middle tier (architecture.md §4.6).

---

## 4. Google generative-ai — `mod google_genai` (new dialect + a HeaderSpec proof)

Google's `generateContent` / `streamGenerateContent` (`POST {base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse`, SSE) is a new dialect (`contents[]`/`parts[]`/`functionCall`) **and** the proof that a new auth-header name (`x-goog-api-key`) is **pure row data, not code**. Cost: **one module + one arm + one insert (+ a row)**. **No new `Auth` impl.**

### 4.1 Cost & the provider row — `x-goog-api-key` is DATA

```rust
protocols.insert(ProtocolId::GoogleGenAi, &GoogleGenAi);   // ONE insert
enum ProtocolId { /* … */ GoogleGenAi }                    // ONE arm
```
```toml
[[provider]]
name = "google"
base_url = "https://generativelanguage.googleapis.com"
protocol = "google_generative_ai"
auth = "api_key"                                            # reuses ApiKeyAuth — NO new Auth impl
api_header = { name = "x-goog-api-key", scheme = "raw" }    # the entire "Google auth header" diff: DATA
```

**The HeaderSpec proof (architecture.md §4.6).** Google authenticates with a custom header name (`x-goog-api-key`) carrying the raw key. This is already expressible as `HeaderSpec { name: "x-goog-api-key", scheme: Raw }` on the row. `ApiKeyAuth::apply` reads `ctx.api_header` (data) and sets the named header to the raw secret (architecture.md §4.5, §7) — **no branch on "is this Google", no new `Auth` impl, no `AuthId` arm.** It is the identical mechanism that sets Anthropic's `x-api-key` (also `scheme: Raw`); only the `name` field of the data differs. Auth cost: **zero code, one field of one row.**

`framing(&self) -> Framing { Framing::Sse }` (with `?alt=sse`, Google emits SSE; the default JSON-array streaming form is not used). The model id is a **path segment** (`models/{model}:streamGenerateContent`), so `encode` builds the URL from `ctx.model` — a URL-shape difference absorbed entirely in `encode`, not a new seam.

### 4.2 REQUEST mapping — canonical → `generateContent` wire

| Canonical | Google wire | Rule |
|---|---|---|
| `model` | URL path `models/{ctx.model}` | alias-resolved; selects the endpoint, not a body field. |
| `system: Option<Vec<Content>>` | `"systemInstruction": {parts:[{text}]}` | text-only top-level. `None`/empty → omit. Non-`Text` → `Error{ParseInput}`/64 (architecture.md §3.1). |
| `messages` | `"contents"` (array of `{role, parts:[…]}`) | §4.3. Google roles are `"user"`/`"model"`. |
| `tools` | `"tools":[{functionDeclarations:[{name, description?, parameters}]}]` | **omit when empty.** `parameters` ← `input_schema` (an OpenAPI-subset schema `Value`, passed through). |
| `tool_choice` | `"toolConfig":{functionCallingConfig:{mode}}` | `Auto`→`AUTO` (or omit); `Any`→`ANY`; `None`→`NONE`; `Tool{name}`→`ANY`+`allowedFunctionNames:[name]`. |
| `max_tokens` | `generationConfig.maxOutputTokens` | `Some`→value; `None`→omit. |
| `temperature`/`top_p` | `generationConfig.temperature`/`.topP` | `Some`→value; `None`→omit. |
| `stop: Vec<String>` | `generationConfig.stopSequences` | **RENAME + nesting.** omit when empty. |
| `stream` | — | streaming is the **endpoint choice** (`:streamGenerateContent` vs `:generateContent`), not a body field — selected from `req.stream` in `encode`. No `stream` key on the wire. |
| `extra` | merged into the body (typically under `generationConfig`/`safetySettings`) | the valve: `safetySettings`, `topK`, `responseMimeType`, `responseSchema`, `cachedContent`, `thinkingConfig`, … Typed fields win (architecture.md §3.1). |

#### 4.3 `contents[]` — per-`Message` projection

Google has **no system or tool role**; roles are `user`/`model`. The adapter owns the projection (architecture.md §3.1):

| Canonical `Role` / `Content` | Google wire |
|---|---|
| `Role::User` | `{role:"user", parts:[…]}` |
| `Role::Assistant` | `{role:"model", parts:[…]}` |
| `Role::System` *(in messages)* | hoisted to `systemInstruction` (like `req.system`); never an inline content (mirrors Anthropic's hoist, anthropic-messages.md §2.3) |
| `Role::Tool` | `{role:"user", parts:[{functionResponse:…}]}` — Google carries tool results in a user turn (adapter projection, as Anthropic does) |
| `Content::Text(s)` | `{text:s}` |
| `Content::Image{Base64{mt,data}}` | `{inlineData:{mimeType:mt, data}}` — **structured** base64 (unlike OpenAI's data-URI); round-trips cleanly |
| `Content::Image{Url{url}}` | `{fileData:{fileUri:url}}` |
| `Content::ToolUse{id,name,input}` | `{functionCall:{name, args:input}}` — `args` is a JSON **object** (not a string). **Google sends no tool-call id** → see §4.5 |
| `Content::ToolResult{tool_use_id, content, is_error}` | `{functionResponse:{name, response:{…}}}` — keyed by **name**, not id (§4.5); text-only-ish slot, non-`Text` → `Error{ParseInput}`/64. `is_error` surfaced textually |
| `Content::Thinking` / `RedactedThinking` | thought signatures ride `extra`/`thinkingConfig`; dropped on plain re-send (empty-set rule; §7 CR-G2) |

### 4.4 RESPONSE mapping — `streamGenerateContent` SSE → canonical `Vec<Event>`

Each SSE `data:` frame is a `GenerateContentResponse` chunk: `{candidates:[{content:{role:"model", parts:[…]}, finishReason?, index}], usageMetadata?}`. There is **no per-block start/stop on the wire** — the adapter synthesizes the canonical block structure.

| Wire feature (per chunk, `candidates[0]`) | Canonical events | DecodeState action |
|---|---|---|
| first chunk | `MessageStart{ id: None, model: Some(modelVersion?), role: Assistant }` once (gated `started`) | `started = true`. (Google streams no message id → `id: None`, never fabricated, architecture.md §3.2.) |
| first `parts[].text` | **synthesize** `ContentStart{i, Text {}}` then `ContentDelta{i, TextDelta(text)}` | open text block at `i = next_index++` |
| subsequent `parts[].text` | `ContentDelta{i, TextDelta(text)}` | — |
| `parts[].functionCall{name, args}` (arrives **whole**, not fragmented) | **synthesize** `ContentStart{c, ToolUse{ id: synth, name }}`, then one `ContentDelta{c, JsonDelta(to_json_string(args))}`; the block stays open and **closes at the terminal drain** (the `finishReason` chunk below) | assign canonical index (monotonic `open.len()`); id synthesized (§4.5) |
| `parts[].thought` text | `ContentStart{Thinking {}}`/`ThinkingDelta`/`ContentStop` (when `thinkingConfig` surfaces thoughts) | — |
| chunk carrying non-null `finishReason` (the **last chunk**) | drain open blocks → `ContentStop*`; then `Usage`(§4.6); then `Finish{reason}`(§4.7); then `[]` and **`state.terminated = true`** | the native terminator |
| `usageMetadata` on any chunk | `Usage` (§4.6) | cumulative |
| `promptFeedback.blockReason` / `finishReason=="SAFETY"` | `Finish{Refusal{category: blockReason\|"safety", explanation}}` (HTTP 200, exit 0 — architecture.md §3.2) | — |

**The terminator (architecture.md §3.4).** Google sends **no `[DONE]` and no `message_stop`** — the **last chunk carries a non-null `finishReason`**. `decode` recognizes that chunk as the terminal marker: it drains, emits `Usage`/`Finish`, and sets `state.terminated = true`. `decode` **never emits `End`**; `run` appends the one `End` at body EOF (architecture.md §4.4). A subsequent clean SSE EOF with `terminated` set suppresses the premature-EOF injection (architecture.md §5.6, CR-9). This is the one terminator whose marker is a *field on the content chunk* rather than a standalone sentinel frame — but the `terminated`-bit discipline is identical to every other protocol.

### 4.5 Tool-call id is synthesized (Google sends none)

Google's `functionCall` carries **no id** (results are matched by function `name`). The canonical model requires `ContentStart{ToolUse{id, name}}` with a non-empty `id` so that identity-before-content holds and a folding consumer can key the call (architecture.md §3.2, §3.6). The adapter **synthesizes a deterministic id** — `"call_{candidateIndex}_{block_index}"` from `DecodeState` — so the canonical event shape is satisfied. On the request side, `ToolResult.tool_use_id` is projected back to `functionResponse.name` (the synthesized id is not re-sent; Google keys by name). This is an adapter-owned projection, not a canonical change (architecture.md §3.1). See §7 CR-G1 — recorded as a watch item, no architecture change required (the empty-set/synthesis pattern matches OpenAI's synthesized `ContentStart`).

### 4.6 `Usage` mapping

`usageMetadata`: `promptTokenCount`→`input`, `candidatesTokenCount`→`output`, `cachedContentTokenCount`→`cache_read` (`Some` iff present), no cache-write → `cache_write: None`. `totalTokenCount`/`thoughtsTokenCount` derivable/long-tail → dropped. `Option` throughout — absent field is `None`, never `0` (architecture.md §3.2).

### 4.7 `finishReason` → `FinishReason`

| wire `finishReason` | `FinishReason` |
|---|---|
| `STOP` | `Stop` |
| `MAX_TOKENS` | `Length` |
| `SAFETY` / `PROHIBITED_CONTENT` / `BLOCKLIST` | `Refusal{category: <reason lowercased>, explanation: <safetyRatings summary?>}` (HTTP 200, exit 0) |
| *(a `functionCall` part present)* | `ToolUse` (Google reports `STOP` even on a tool call; the adapter promotes to `ToolUse` when the candidate contains a `functionCall` part — mirrors the canonical intent) |
| `RECITATION` / `OTHER` / unknown `s` | `Other(s)` — never panics (architecture.md §9.5) |

`StopSequence` is not distinctly reported by Google (a stop-sequence hit is `STOP`) → `Stop`; provider-inherent, excluded from any cross-check (as openai-chat-mapping.md §3.5).

### 4.8 ERROR mapping

Non-2xx whole-body frame (shared decoder contract); Google's envelope is `{"error":{"code":<int>,"message":"…","status":"…","details":[…]}}`. `decode` emits `Event::Error{kind, message: error.message, provider_detail: Some(error)}`; the **HTTP status** drives kind+exit per the shared table (architecture.md §8). `error.status` (e.g. `RESOURCE_EXHAUSTED`, `PERMISSION_DENIED`) is informational, rides `provider_detail`. A mid-stream error chunk is exited by decoded `kind` (architecture.md §8, CR-10), same discipline as §3.7.

### 4.9 What's NOT touched (severability)

`run`, `resolve`, `parse`, `Sink`, the canonical model, the other Protocol impls, **and `ApiKeyAuth`** are unchanged. The `x-goog-api-key` need was met by one data field. `finishReason`-bearing-last-chunk normalizes to the same `Event::End`. Delete module + arm + insert → gone (the row's `auth = "api_key"` data is harmless; only the `protocol` arm is removed). Rubric tiers exercised: a new dialect (module) **and** the HeaderSpec-is-data proof.

---

## 5. Ollama — `mod ollama_chat` (new dialect, **NDJSON framing**)

Local Ollama (`POST {base_url}/api/chat`, **NDJSON** — newline-delimited JSON, one object per line, **not SSE**). Cost: **one module + one arm + one insert (+ a row)**. Its distinctive contribution is `framing() -> Framing::Ndjson` and the `{"done":true}` terminator.

### 5.1 Cost & the provider row

```rust
protocols.insert(ProtocolId::OllamaChat, &OllamaChat);   // ONE insert
enum ProtocolId { /* … */ OllamaChat }                    // ONE arm
```
```toml
[[provider]]
name = "ollama"
base_url = "http://localhost:11434"
protocol = "ollama_chat"
auth = "bearer"                                            # local Ollama ignores it; a bearer is set if a key exists (harmless)
api_header = { name = "Authorization", scheme = "bearer" }
```

Local Ollama needs no auth; `BearerAuth` sets `Authorization: Bearer …` only if a credential/inline key is present, otherwise `ApiKey`-style "no creds" would 77 — so the row uses `bearer` and a **missing local key is tolerated** because Ollama ignores the header. (An operator pointing at a gated remote Ollama supplies a key via the normal cred path; no code difference.) `default_max_tokens` is **not** set — Ollama does not require it.

### 5.2 `framing()` — the one mechanical difference (NDJSON, not SSE)

```rust
fn framing(&self) -> Framing { Framing::Ndjson }
```

`Framing::Ndjson` selects the shared **NDJSON line-framer** (defined in the SSE-decoder spec / `protocol/sse.rs` per architecture.md §11; **this spec does not redefine it**). The framer yields **one `Frame` per `\n`-terminated line**, each a complete JSON object; partial-line buffering across transport chunks (and adversarial rechunking — `OneByte`/`MidUtf8`/`MidJsonNumber`, architecture.md §9.3) is the framer's job, exactly as SSE partial-frame buffering is. `decode` only ever sees a complete frame. This is the entire framing cost: a **data** return value (`Framing` is DATA, not behaviour — architecture.md §4.1), routed by `run`'s `framing.decoder()` (architecture.md §4.4). No new framer code, no branch in `run`.

### 5.3 REQUEST mapping — canonical → Ollama `/api/chat` wire

Ollama's chat body is OpenAI-chat-shaped with Ollama-specific nesting of generation params under `options`:

| Canonical | Ollama wire | Rule |
|---|---|---|
| `model` | `"model"` | `ctx.model` (alias-resolved). |
| `system: Option<Vec<Content>>` | leading `messages[0]` `{role:"system"}` (text) | as OpenAI chat (openai-chat-mapping.md §2.3); non-`Text` → `Error{ParseInput}`/64. |
| `messages` | `"messages"` (`{role, content, images?, tool_calls?}`) | §5.4. |
| `tools` | `"tools":[{type:"function", function:{name, description?, parameters}}]` | OpenAI-chat shape; omit when empty. |
| `tool_choice` | — | **no native field;** Ollama infers from `tools`. A required-tool intent rides `extra` if the model supports it (watch item, §7 CR-O1). |
| `max_tokens` | `options.num_predict` | **RENAME + nesting under `options`.** `Some`→value; `None`→omit. |
| `temperature`/`top_p` | `options.temperature`/`options.top_p` | nested under `options`; `None`→omit. |
| `stop: Vec<String>` | `options.stop` | nested; omit when empty. |
| `stream` | `"stream"` | the bool. `false` → a single NDJSON object (the folded stream, architecture.md §3.2). |
| `extra` | merged top-level / into `options` | the valve: `keep_alive`, `format` (JSON-mode/schema), `options.*` knobs (`num_ctx`, `seed`, `repeat_penalty`, …). Typed fields win (architecture.md §3.1). |

#### 5.4 `messages[]` projection

| Canonical `Content` | Ollama wire |
|---|---|
| `Text(s)` | message `content` string (concatenated for multi-text) |
| `Image{Base64{_,data}}` | `images:[<base64>]` — a **bare base64 array** on the message (Ollama drops the media-type; the raw base64 is the payload) |
| `Image{Url{..}}` | **UNREPRESENTABLE** — Ollama takes base64 only; a URL image → `Error{ParseInput}`/64 (a documented text/base64-only-slot rejection, architecture.md §3.1) |
| `ToolUse{id?,name,input}` | assistant `tool_calls:[{function:{name, arguments:input}}]` — `arguments` an **object** (Ollama, unlike OpenAI, takes a JSON object, not a string). Ollama sends no tool-call id → synthesized on decode (§5.6) |
| `ToolResult{tool_use_id,content,is_error}` | `{role:"tool", content:<text>}` — keyed positionally (no id field); text-only, non-`Text` → `Error{ParseInput}`/64 |
| `Thinking{text,..}` | `{role:"assistant", thinking:<text>}` if the model supports `think` (ride `extra`), else dropped (empty-set rule) |
| `RedactedThinking` | dropped — never produced by this adapter |

`Role::Tool` → `{role:"tool"}` (Ollama has a tool role, like OpenAI). `Role::System` → `{role:"system"}`.

### 5.5 RESPONSE mapping — Ollama NDJSON → canonical `Vec<Event>`

Each line is `{model, created_at, message:{role:"assistant", content, images?, tool_calls?, thinking?}, done:bool, done_reason?, <stats?>}`:

| Wire feature (per line) | Canonical events | DecodeState action |
|---|---|---|
| first line | `MessageStart{ id: None, model: Some(model), role: Assistant }` once | `started = true` (Ollama streams no message id → `id: None`) |
| first non-empty `message.content` | **synthesize** `ContentStart{i, Text {}}` then `ContentDelta{i, TextDelta(content)}` | open text block |
| subsequent `message.content` | `ContentDelta{i, TextDelta(content)}` | — |
| `message.tool_calls[]` (each arrives **whole** — name+args together, not fragmented) | **synthesize** `ContentStart{c, ToolUse{id:synth, name}}`, one `ContentDelta{c, JsonDelta(to_json_string(args))}`; the block stays open and **closes at the terminal drain** (the `done:true` line below) | assign index (monotonic `open.len()`); synth id (§5.6) |
| `message.thinking` | `ContentStart{Thinking {}}`/`ThinkingDelta`/`ContentStop` (when `think` enabled) | — |
| `"done": true` (the terminal line; carries `done_reason` + final token stats) | drain open blocks → `ContentStop*`; `Usage`(§5.7); `Finish{reason}`(§5.8); then `[]` and **`state.terminated = true`** | the native terminator |
| `"done": false` | only the content/tool events above | — |

**The terminator (architecture.md §3.4).** `{"done": true}` is Ollama's native terminator. The terminal line decodes its `Usage`+`Finish`, then `decode` sets `state.terminated = true` and returns. `decode` **never emits `End`**; `run` appends the one `End` at body EOF (architecture.md §4.4). Because Ollama's terminal marker is a **field (`done`) on the final content line** (like Google's `finishReason`, not a standalone sentinel like `[DONE]`/`message_stop`), the same content line may carry both the last content delta *and* `done:true` — the adapter emits the content events first, then drains/finishes. `terminated` set → premature-EOF injection suppressed (architecture.md §5.6, CR-9). One `End` in all cases, identical to every sibling protocol.

### 5.6 Tool-call id synthesized; tool args arrive whole

Like Google (§4.5), Ollama sends **no tool-call id** and emits tool-call `arguments` as a **complete object on one line**, not as streamed fragments. The adapter synthesizes a deterministic id (`"call_{n}"`, where `n` is the canonical index from `DecodeState`) for `ContentStart{ToolUse{id,name}}` (identity-before-content, architecture.md §3.2), then emits the whole arguments object as a **single** `Delta::JsonDelta(to_json_string(args))`. The block is then left open and its `ContentStop` is emitted at the **terminal drain** — the same single drain point and monotonic `open.len()` index discipline OpenAI uses (so the index is never stored; openai-chat-mapping.md §3.1). The "fragments are valid only concatenated" rule (architecture.md §3.6) is satisfied trivially by a one-fragment stream — the consumer's assembly+parse is unchanged. No mid-stream parse happens in `decode`.

### 5.7 `Usage` mapping

The terminal `done:true` line carries token stats: `prompt_eval_count`→`input`, `eval_count`→`output`. Ollama reports **no cache fields** → `cache_read: None`, `cache_write: None` (never `0`, architecture.md §3.2). Durations (`total_duration`, `eval_duration`, …) have no canonical home → dropped (ride `provider_detail` only under a future need). `Option` throughout.

### 5.8 `done_reason` → `FinishReason`

| wire `done_reason` (on the `done:true` line) | `FinishReason` |
|---|---|
| `"stop"` | `Stop` |
| `"length"` | `Length` |
| *(a `tool_calls` present in the turn)* | `ToolUse` |
| absent (older Ollama: `done:true` with no `done_reason`) | `Stop` (the default normal completion) |
| any other string `s` | `Other(s)` — never panics (architecture.md §9.5) |

Ollama exposes no refusal channel and no distinct stop-sequence reason → `Refusal`/`StopSequence`/`Pause` are not produced (the empty-set rule; a stop hit is `Stop`).

### 5.9 ERROR mapping

A non-2xx body is a single JSON object `{"error":"<message>"}` (a bare string, not OpenAI's nested envelope) — reached via the shared whole-body-frame decoder contract (the framer hands the whole non-2xx body as one frame regardless of `Ndjson` vs `Sse`, openai-chat-mapping.md §4.0). `decode` emits `Event::Error{kind, message: error, provider_detail: Some({"error": error})}`; the **HTTP status** drives kind+exit per the shared table (architecture.md §8). Local Ollama failures are usually `Transport`/69 (connection refused — produced by the transport seam, not `decode`) or a `Provider{4xx}` for a bad request (model not pulled → 404 → `Provider{404}`/69).

### 5.10 What's NOT touched (severability)

`run`, `resolve`, `parse`, `Sink`, the canonical model, the other Protocol impls, **and the NDJSON line-framer** (it already exists for the `--json` output `Sink` and is shared, architecture.md §5.2, §11) are unchanged. `framing() -> Ndjson` is a DATA return routed by `run`'s existing `framing.decoder()` switch (architecture.md §4.4) — `run` does not branch on the framing kind beyond that one data-driven `decoder()` call. `{"done":true}` normalizes to the same `Event::End`. Delete module + arm + insert → gone.

---

## 6. The severability ledger (the executable grading rubric)

Per architecture.md §4.6 — the exact cost of each addition, and the confirmation that the core is untouched:

| Addition | Rows | `ProtocolId` arms | Registry inserts | Modules | `Auth` impls | Core touched? |
|---|---:|---:|---:|---:|---:|---|
| **Mistral** | **1** | 0 | 0 | **0** | 0 | **No** — reuses `OpenAiChat` + `BearerAuth` verbatim |
| **OpenAI responses** | 1 | 1 | 1 | 1 | 0 | **No** |
| **Google generative-ai** | 1 | 1 | 1 | 1 | **0** (`x-goog-api-key` is row DATA read by `ApiKeyAuth`) | **No** |
| **Ollama** | 1 | 1 | 1 | 1 (`framing() -> Ndjson`) | 0 | **No** |

"Core" = `run`, `resolve`, `parse`, the `Sink`, the canonical model (architecture.md §3 types), and every other `Protocol` impl. **None of these changes for any row above.** The proof, executable:

- **One terminator.** `response.completed` (§3.4), the `finishReason`-bearing last chunk (§4.4), and `{"done":true}` (§5.5) **all** normalize to the **same single `Event::End`** that `[DONE]` and `message_stop` produce — `run` appends it once at body EOF, no `decode` ever emits it, and each sets the **same** `state.terminated` bit that gates the **same** premature-EOF injection (architecture.md §3.4, §4.4, §5.6, CR-9). The "is the stream over?" question has one answer for all five protocols.
- **No match-on-provider.** Every addition is reached as `cfg.provider.protocol` / `cfg.provider.auth` **map keys** (architecture.md §4.4); `name` reaches no dispatch site. Mistral proves the floor (data only); Google proves a new auth-header name is data, not code.
- **Deletion is clean.** Delete a row → that provider is gone (Mistral leaves nothing behind). Delete a module + its arm + its insert → that dialect is gone; rows that named it fail at `resolve` with a `Config` error (78), never a silent mis-decode (architecture.md §4.6).

---

## 7. Edge cases & architecture change requests

Per the derivation rule (architecture.md §1 of each mapping spec): nothing is silently deviated. Each gap is resolved here, resolved-in-architecture, or **deferred** as a genuine open item. The shared, already-resolved-in-architecture items (`extra` precedence with typed fields winning; the non-text-slot `ParseInput`/64 rejection; externally-tagged `ContentKind`/`Delta` serde; `DecodeState.terminated`/premature-EOF; post-200 mid-stream exit-by-`kind`) apply to **every** protocol here exactly as in the sibling mappings — not re-litigated.

### Resolved here (no canonical change)

- **Mistral wire deviations** (§2.3): every one fits in `extra` / an empty-set path / a row signal. Zero code. The severability proof.
- **Synthesized tool-call ids for Google & Ollama** (§4.5, §5.6): both wires send no id; the adapter synthesizes a deterministic id to satisfy `ContentStart{ToolUse{id,name}}` (architecture.md §3.2), the same synthesis pattern OpenAI chat already uses for its `ContentStart`. On re-send, `ToolResult` is keyed back by `name`/position. Adapter-owned projection, no canonical change.
- **Whole (non-fragmented) tool args for Google & Ollama** (§4.4, §5.6): emitted as a **single** `JsonDelta`, the block then closing at the terminal drain (so `open.len()` stays a monotonic, never-stored canonical index — the same drain discipline OpenAI uses); the "valid only concatenated" rule (architecture.md §3.6) holds trivially for one fragment.
- **Field-on-content-chunk terminators** (Google `finishReason`, Ollama `done:true`): unlike the standalone sentinels (`[DONE]`/`message_stop`/`response.completed`), the marker rides the final *content* line. The adapter emits that line's content events first, then drains + finishes + sets `terminated`. The `terminated`-bit discipline (architecture.md §5.6, CR-9) is identical regardless of marker shape. No change.
- **`Framing::Ndjson` for Ollama** (§5.2): `Framing` is DATA (architecture.md §4.1); the NDJSON line-framer already exists (the `--json` `Sink`, architecture.md §5.2). One data return value, no new framer, no `run` branch. No change.
- **`x-goog-api-key` as `HeaderSpec` data** (§4.1): expressible today (architecture.md §4.6); `ApiKeyAuth` reads `ctx.api_header`. No new `Auth`, no change.

### Deferred / watch items (genuine gaps — recorded, not silently worked around)

- **CR-R1 — Responses API has no top-level `stop` field.** Canonical `stop` (a typed field) has no direct `responses` wire home (§3.2). v0.1: a non-empty `stop` is omitted unless the caller supplies the wire key via `extra`. *Status:* a documented narrowing in the encode direction (like openai-chat-mapping.md's dropped `Thinking` on re-send), not a canonical change. Raised only if `stop` on Responses becomes load-bearing. **Low urgency.**
- **CR-R2 / G-equiv / O-equiv — `ToolResult.is_error` has no native field** on Responses (`function_call_output`), Google (`functionResponse`), or Ollama (`tool` message). Surfaced **textually** (content prefix), identical to the OpenAI chat degradation (openai-chat-mapping.md §6 CR-3). The structured boolean does not round-trip. Same deferred CR as the OpenAI mapping — one resolution covers all dialects.
- **CR-R3 — Responses reasoning replay** rides `previous_response_id` / encrypted reasoning items, out of scope for v0.1; `Content::Thinking` is dropped on plain re-send (§3.3). Consistent with the empty-set rule and openai-chat-mapping.md CR-2. **Low urgency.**
- **CR-G1 — Google tool-call id absence** (§4.5): synthesized id is an adapter projection; recorded as a watch item only — **no architecture change required** (matches the OpenAI synthesized-`ContentStart` pattern).
- **CR-G2 — Google thought signatures** (`thinkingConfig`/`thoughtSignature`) ride `extra`; not normalized as a canonical signature in v0.1. The Anthropic `Thinking.signature` slot is the canonical home for verbatim signatures (architecture.md §3.1); whether Google's thought-signature replay maps onto it is **deferred** until Google multi-turn thinking replay is required. **Low urgency.**
- **CR-O1 — Ollama has no `tool_choice` field** (§5.3): a required-tool intent (`ToolChoice::Any`/`Tool{name}`) has no native Ollama spelling; v0.1 omits it (Ollama infers from `tools`). Raised only if forcing a tool call on Ollama becomes load-bearing. **Low urgency.**
- **CR-O2 — Ollama URL-image rejection** (§5.4): Ollama takes base64 only, so `Image{Url}` → `Error{ParseInput}`/64 — a documented base64-only-slot rejection (architecture.md §3.1), the image analogue of the text-only-slot rule. No change.

### Cross-spec consistency (relied on, not a change)

- **All four protocols here uphold the same invariants the sibling mappings pin** (§1.1): typed fields win over `extra`; provider 4xx→69 / 5xx→70 / 401-403→77 (architecture.md §8); `decode` never emits `End` and sets `terminated` on the terminal marker; `Usage` fields `Option` (never fabricated `0`); refusal is `Finish{Refusal}` at exit 0. These are the consistencies that let any future cross-protocol equality test (architecture.md §3.6) be writable — not architecture changes.

---

## 8. Summary of decisions (this spec is decisive)

- **Mistral** = one `[[provider]]` row on `protocol="openai_chat"`+`auth="bearer"`, **zero Rust**; deletes cleanly; every wire deviation fits in `extra`/empty-path/row data. The severability floor.
- **OpenAI responses** = `mod openai_responses` + `ProtocolId::OpenAiResponses` arm + one insert + a row. `Framing::Sse`. `system`→`instructions`, `messages`→`input[]`, `max_tokens`→`max_output_tokens`; `response.completed`→`Usage`+`Finish`+`terminated`; `run` appends the one `End`.
- **Google generative-ai** = `mod google_genai` + one arm + one insert + a row whose `api_header = { name="x-goog-api-key", scheme="raw" }` is the **HeaderSpec-is-data proof** (no new `Auth`). `Framing::Sse`. Model in the URL path; roles `user`/`model`; structured `inlineData` images; **last chunk's non-null `finishReason`** is the terminator → `terminated`.
- **Ollama** = `mod ollama_chat` + one arm + one insert + a row. **`framing() -> Framing::Ndjson`** (the distinctive cost; the line-framer is shared, not redefined). Params nested under `options`; tool args & ids synthesized whole; **`{"done":true}`** is the terminator → `terminated`.
- **One terminator for all.** `response.completed` / `finishReason`-bearing-last-chunk / `{"done":true}` all normalize to the **same single `Event::End`** appended once by `run`; `decode` never emits it; each sets the same `terminated` bit gating the same premature-EOF injection (architecture.md §3.4, §4.4, §5.6).
- **No core change for any addition.** `run`/`resolve`/`parse`/`Sink`/canonical model/other Protocol impls are untouched; dispatch is by `ProtocolId`/`AuthId` map keys, never a vendor name (architecture.md §4.4, §4.6).

CITATIONS: https://platform.openai.com/docs/api-reference/responses · https://platform.openai.com/docs/api-reference/responses-streaming · https://ai.google.dev/api/generate-content · https://ai.google.dev/gemini-api/docs/text-generation · https://github.com/ollama/ollama/blob/main/docs/api.md · https://docs.mistral.ai/api/
