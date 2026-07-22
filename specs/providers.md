# Provider rows — Mistral, OpenAI responses, Google generative-ai, Ollama

> **Living document.** Edited like code. This spec is a set of **lossy projections** onto and back from the canonical model of architecture.md; it MUST NOT contradict it. Where a wire dialect cannot express a canonical fact (or vice-versa), this spec raises a **change request to architecture.md** (§9) rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — especially §3 (the canonical model, the single source of truth each dialect projects onto/from), §3.4 (the native-terminator→`End` table), §4.1 (the `Protocol` trait, `ProviderCtx`, `HeaderSpec`, `Framing`), §4.2 (Provider is DATA — the embedded TOML rows), §4.4 (dispatch with no match-on-provider), §4.6 (the severability proof), §11 (a new protocol = one module).
> **Sibling mapping specs (referenced, not duplicated):** [OpenAI chat](openai-chat-mapping.md) · [Anthropic messages](anthropic-messages.md). NDJSON framing and `Frame`/`DecodeState` mechanics live in the SSE-decoder spec; this spec cites the framing **contract** (architecture.md §3.4/§4.1) and never redefines the framer.

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
2. **Every impl is vendor-blind** — it reads only `ProviderCtx { base_url, model (alias-resolved), beta_headers }`; the vendor name was spent on the registry lookup before `encode` runs, and the auth header rides `AuthCtx`, not `ProviderCtx` (architecture.md §4.1). Config body passthrough rides `req.extra` (seeded by `fill_absent`), not a `ctx.extra` (config §4.1).
3. **Auth is not Protocol** — `encode` sets only body + non-auth headers; the auth header is set by `Auth::apply` reading `auth.api_header` as DATA (architecture.md §4.5).
4. **`content` is ALWAYS `Vec<Content>`**; a bare wire string decodes to `vec![Content::Text(..)]`.
5. **Identity precedes content** — `ContentStart{index, kind}` (carrying tool id/name) is emitted before any `ContentDelta` for that index; an adapter whose wire lacks a block-open **synthesizes** it (architecture.md §3.2, §3.6).
6. **Tool-call arguments stream as `Delta::JsonDelta(String)` fragments** — never parsed mid-stream; parsed to a `Value` only when folding to `Content::ToolUse` (architecture.md §3.6).
7. **Exactly ONE `Event::End` per response.** `decode` **NEVER emits `End`** — the single terminator is the `sink.write(&Event::End)` the `run` loop appends once after the body iterator drains (architecture.md §4.4). Each protocol's terminal marker decodes to `[]` and sets `state.terminated = true` (architecture.md §3.5, CR-9), suppressing the premature-EOF injection (architecture.md §5.6).
8. **Refusal is a `Finish{Refusal}`, never an `Error`** (architecture.md §3.2); HTTP 200, exit 0. `Error` is its own event, never folded into `Finish` (architecture.md §3.3).
9. **`Usage` fields are `Option`** — `None` is "unknown", never a fabricated `0` (architecture.md §3.2).
10. **`decode` is pure over `(frame, &mut DecodeState)`**; provider-error parsing lives in `decode`, the HTTP status is peeked separately for the exit code (architecture.md §8).

The HTTP-status→`ErrorKind`→exit table (architecture.md §8: provider 4xx→69 incl. 429, 5xx→70, 401/403→77, malformed-stdin→64) and the non-2xx whole-body-frame decoder contract (openai-chat-mapping.md §4.0, anthropic-messages.md §4.0) are **shared by every protocol below** — each error section names only its dialect's error-envelope shape and defers the status→exit mapping to that shared table. On that same non-2xx handshake the **`Retry-After` response header** — when present, regardless of dialect — is carried into `CanonicalError.retry_after_seconds` (whole seconds; integer or `HTTP-date` form, architecture.md §3.3), the transport pacing hint for a caller's retry loop; it is a response-header fact, so it lives outside every `decode` (stamped by `run`, not the per-dialect envelope parse) and needs no per-protocol mention.

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

`protocol = "openai_chat"` is a **registry key**, not a dispatch branch (architecture.md §4.2, §4.4). At resolution time `cfg.provider.protocol == ProtocolId::OpenAiChat`, so `run` looks up `registry.protocols[&ProtocolId::OpenAiChat]` — the **same `&OpenAiChat` impl** that serves the `openai` row. `encode`/`decode`/`framing` run unchanged; they never learn the provider is Mistral because `ProviderCtx` carries **no name** (architecture.md §4.1). `auth = "bearer"` likewise reuses `&BearerAuth`, which reads the header to set (`Authorization: Bearer …`) from `auth.api_header` as DATA (architecture.md §4.5, §7). The request/response/error mapping is **exactly** openai-chat-mapping.md §2–§4 — nothing here re-specifies it.

**The deletion test (architecture.md §4.6).** Delete the four-line row → Mistral is gone, cleanly, with no dangling code, because there was never any Mistral code. A request naming `--provider mistral` then resolves to no row → `Config` error, exit 78 (architecture.md §4.3). No module, no enum arm, no insert touched. This is the lower bound of the rubric: **adding a provider that reuses an existing protocol+auth is pure data.**

### 2.2 Routing & aliases

The row ships **no** `model_aliases` and **no** `body_defaults` — Mistral Chat Completions does not require `max_tokens` (so `req.max_tokens` stays `None` and is omitted, openai-chat-mapping.md §2.1), and alias tables are optional shorthand (architecture.md §4.3). A user routes to Mistral by `--provider mistral`, or by adding aliases in their own config file (the file layer is the same `PartialConfig` schema, architecture.md §6.1) — an operator concern, not a built-in. Identity passthrough (`model_aliases.get(model).unwrap_or(model)`, architecture.md §4.3) means an unaliased Mistral wire id (`mistral-large-latest`) passes through verbatim once the provider is named.

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
| `parallel_tool_calls: Option<bool>` | `"parallel_tool_calls"` | `Some(b)`→top-level bool; `None`→omit. The wire SUPPORTS it, so it is emitted TOP-LEVEL exactly as Chat Completions (openai-chat-mapping.md §2.6) — the lifted known knob (architecture.md §3.1), not a silent drop (§9 CR-R1). |
| `max_tokens: Option<u32>` | `"max_output_tokens"` | **RENAME.** `Some(n)`→`n`; `None`→omit. The OpenAI row requires none, so normally `None`. (No o-series key issue — Responses always uses `max_output_tokens`, unlike Chat's `max_tokens`/`max_completion_tokens` split, openai-chat-mapping.md §2.7.) |
| `temperature`/`top_p` | `"temperature"`/`"top_p"` | `Some`→value; `None`→omit. **Omitted when `reasoning` is set** — reasoning models (o-series/gpt-5) 400 on non-default sampling; the SAME rule as Anthropic (anthropic-messages.md §2 / §6) and openai_chat (openai-chat-mapping.md §2.7). |
| `reasoning: Option<ReasoningEffort>` | `"reasoning"` (object) + `"include"` (array) | `Some(e)`→`{"effort": e.as_str()}` (`"low"`/`"medium"`/`"high"`) **and** `"include":["reasoning.encrypted_content"]` (bl-61a9); `None`→omit both. Requesting reasoning automatically requests the encrypted reasoning blob back so the harness can replay it statelessly (`store:false`, which the codex/ChatGPT-SSO row mandates) — the "reasoning replay IS the agent loop" thesis, done with zero config rather than a row `body_defaults` `include` the caller must remember. Both written before the `extra` fold, so the typed knobs win over a `body_defaults` `reasoning`/`include` on the same key; a caller needing a different `include` set uses `body_defaults` only when `req.reasoning` is `None`. |
| `stop: Vec<String>` | — | **no native stop field on Responses;** a non-empty `stop` rides `extra` if the caller supplies the wire key, else omitted. (Watch item, §9 CR-R1 — a documented narrowing, not a silent drop of a typed field that *is* supported.) |
| `stream: bool` | `"stream"` | the bool. Responses streams `Usage` natively on `response.completed` — **no `stream_options` knob needed** (unlike Chat Completions, openai-chat-mapping.md §2.8). |
| `extra` (`#[serde(flatten)]`) | merged top-level | the long-tail valve: `text`, `previous_response_id`, `store`, `include`, … and an exact-shape `reasoning` object pinned via `body_defaults` (the §6 escape hatch). Typed fields win (architecture.md §3.1; same precedence as openai-chat-mapping.md §2.1.1) — so a typed `reasoning` knob beats a `body_defaults` `reasoning` object. |

#### 3.3 `input[]` — per-`Message` projection

Each canonical `Message` becomes one or more typed input items. The Responses API uses explicit item types rather than role+content:

| Canonical | Responses input item |
|---|---|
| `Message{User, [Text]}` | `{type:"message", role:"user", content:[{type:"input_text", text}]}` |
| `Message{User, [Image{Base64}]}` | `{type:"input_image", image_url:"data:{mt};base64,{data}"}` (data-URI, as Chat Completions, openai-chat-mapping.md §2.2) |
| `Message{User, [Document{Base64}]}` | `{type:"input_file", filename:<synth>, file_data:"data:{mt};base64,{data}"}` — data-URI `file_data` (as Chat), with `filename` synthesized `document.{subtype}` from the media type (Responses requires it on `file_data`, openai-chat-mapping.md §2.2 CR-C6). **User** turns only. |
| `Message{User, [Document{Url}]}` | `{type:"input_file", file_url:url}` — Responses **fetches web URLs**, so **both** document sources express here, UNLIKE Chat Completions (which rejects the URL, openai-chat-mapping.md §6 CR-C6). Documents are **input-only**; no provider returns one, so no decode side (§9 CR-Doc). |
| `Message{Assistant, [Text]}` | `{type:"message", role:"assistant", content:[{type:"output_text", text}]}` |
| `Content::ToolUse{id,name,input,signature}` | `{type:"function_call", call_id:id, name, arguments:to_json_string(input)}` — `arguments` a JSON **string**, not an object. `signature` (Google `thoughtSignature`) is **ignored** here — Responses `function_call` items carry no signature. |
| `Content::ToolResult{tool_use_id, content, is_error}` | `{type:"function_call_output", call_id:tool_use_id, output:<text>}` — text-only slot; non-`Text` content → `Error{ParseInput}`/64. `is_error` surfaced textually (prefix), no native field (same degradation as openai-chat-mapping.md §2.4, §9 CR-R2) |
| `Content::Thinking{text, signature, id, encrypted_content}` | **when `encrypted_content` is `Some`** (the replayable case): a standalone `{type:"reasoning", id?, summary:[{type:"summary_text", text}]?, encrypted_content}` input item, emitted **before** the message/`function_call` items of the same turn (reasoning precedes what it reasoned about) — bl-61a9, resolving CR-R3. `id` (`rs_…`) is echoed when `Some`; the summary is `[{…text…}]` when `text` is non-empty, else `[]`. **When `encrypted_content` is `None`** (no encrypted blob to replay — e.g. `include` was not requested, or a stateful `store:true` transcript): **dropped** (a bare summary cannot be replayed statelessly). `signature` is Anthropic's, ignored here. |
| `Content::RedactedThinking{data}` | dropped — non-OpenAI variant, never produced by this adapter (the empty-set rule, architecture.md §3.1) |

`Role::System` *in `messages`* projects to `{type:"message", role:"system", ...}` (or `developer` per a row signal, mirroring openai-chat-mapping.md §2.3); `req.system` hoists to `instructions` (§3.2). Both kept distinct (architecture.md §3.1, decision 10).

### 3.4 RESPONSE mapping — `responses` SSE → canonical `Vec<Event>`

The Responses stream is a sequence of typed events. `decode` dispatches on `data.type`. Unlike the synthesized-structure dialects (Google/Ollama), the wire carries explicit block structure, so the **canonical index keys off the wire `(output_index, content_index)` pair** — a single `message` output item can stream several content parts (distinct `content_index`), each its own canonical block, so the bare `output_index` would collide them onto one index. The pair → canonical-index map (`state.part_index`) is assigned on first sight (the map only grows, so its `len` is the next index — the same never-stored discipline `open.len()` gives the synthesized-structure dialects); deltas route by the pair, and `output_item.done` (item-level, carrying only `output_index`) closes **every** block of that item. `function_call` and `reasoning` items carry no `content_index` (the item *is* the block) → pair `(output_index, 0)`, which never collides a message item's parts because the two never share an `output_index`. A `reasoning` item's `reasoning_summary_text.delta`s carry a `summary_index` (not a `content_index`), so they too route by pair `(output_index, 0)` to the one Thinking block — multiple summary parts concatenate into that single canonical block (wire shape verified + collapse-vs-per-part decided in §9 CR-R4):

| Wire `data.type` | Canonical events | DecodeState action |
|---|---|---|
| `response.created` / `response.in_progress` | `MessageStart{ id: Some(response.id), model: Some(response.model), role: Assistant }` **once** (gated on `state.started`) | `started = true` |
| `response.output_item.added` with `item.type=="message"` | — (each text block opens lazily on its `content_part.added`) | — |
| `response.content_part.added` (`part.type=="output_text"`) | **synthesize** `ContentStart{index, Text {}}` | assign the `(output_index, content_index)` pair a canonical index, mark open |
| `response.output_text.delta` (`{delta:"Hel"}`) | `ContentDelta{index, TextDelta(delta)}` | route by the `(output_index, content_index)` pair |
| `response.output_item.added` with `item.type=="function_call"` (carries `call_id`+`name`) | **synthesize** `ContentStart{index, ToolUse{ id: call_id, name }}` — **identity before content** (architecture.md §3.2) | assign pair `(output_index, 0)` a canonical index, mark open |
| `response.function_call_arguments.delta` (`{delta:"{\""}`) | `ContentDelta{index, JsonDelta(delta)}` — **never parsed mid-stream** (architecture.md §3.6) | route by pair `(output_index, 0)` |
| `response.output_item.done` (the item-level close — one per output item) | for a `reasoning` item carrying `encrypted_content`, first `ContentDelta{index, EncryptedReasoningDelta(encrypted_content)}` (bl-61a9 — the encrypted replay blob is revealed on the done item, so it surfaces just before the block's stop; a sink folds it onto `Content::Thinking.encrypted_content`); then `ContentStop{index}` for **every** still-open block of that item, ascending (a multi-part `message` maps to several) | remove each from open. The inner `response.content_part.done` / `response.output_text.done` / `response.function_call_arguments.done` are no-ops (the fragment already streamed); closing on the **outermost** `.done` alone closes each block exactly once |
| `response.output_item.added` with `item.type=="reasoning"` | **synthesize** `ContentStart{index, Thinking { id: item.id }}` — **identity before content** (architecture.md §3.2), mirroring the `function_call` row; `id` (`rs_…`) is captured at open so a `--json` harness can rebuild the reasoning item for replay (bl-61a9) | assign pair `(output_index, 0)` a canonical index, mark open |
| `response.reasoning_summary_text.delta` | `ContentDelta{index, ThinkingDelta(delta)}` — routes by pair `(output_index, 0)` to the Thinking block opened on the `reasoning` item add (the delta carries `summary_index`, no `content_index`) | route by pair `(output_index, 0)` |
| `response.reasoning_text.delta` (raw chain-of-thought) | `ContentDelta{index, ThinkingDelta(delta)}` — the raw reasoning channel (the item's `content[]`, distinct from the `summary[]` channel above); `content_index`-keyed, so `content_index 0` routes by pair `(output_index, 0)` into the same Thinking block (§9 CR-R4 — the summary and raw channels are gated by disjoint model classes and never co-occur in one hosted stream, so they never actually share that block) | route by pair `(output_index, content_index)` |
| `response.reasoning_text.done` / `response.reasoning_summary_text.done` / `response.reasoning_summary_part.added` / `response.reasoning_summary_part.done` | **no-op** — the inner reasoning `.done`/`.part` family, mirroring the `content_part.done` / `output_text.done` no-ops above: the Thinking block is closed exactly once by the outermost `response.output_item.done`, never by an inner `.done`, and the fragment already streamed via the matching `.delta`. `reasoning_summary_part.added` is the per-part OPEN seam CR-R4 names for a future per-part Thinking block; until a real consumer needs it the one-block collapse holds, so it opens nothing (§9 CR-R4) | fall through, yield `[]` |
| `response.completed` | `Usage`(from `response.usage`, §3.5) then `Finish{reason}` (§3.6); then `[]` and **`state.terminated = true`** | drain any still-open blocks to `ContentStop` first |
| `response.incomplete` | `Finish{Length}` if `incomplete_details.reason=="max_output_tokens"`, else `Finish{Other(reason)}`; sets `terminated` | drain open blocks |
| `response.error` / `response.failed` | `Error(CanonicalError{..})` (§3.7); no `End` | mid-stream error, terminal |
| `response.refusal.delta` / refusal item | accumulate; surfaced as `Finish{Refusal{category:"refusal", explanation:Some(acc)}}` at completion (HTTP 200, exit 0 — architecture.md §3.2) | append to `state.refusal` |

**The terminator (architecture.md §3.4).** `response.completed` (the native terminator) decodes its `Usage`+`Finish` and then sets `state.terminated = true`; `decode` **never emits `End`** — `run` appends the single `End` at body EOF (architecture.md §4.4). Because `terminated` is set, the premature-EOF injection is suppressed (architecture.md §5.6, CR-9). Identical End-ownership discipline to openai-chat-mapping.md §3.6 and anthropic-messages.md §3.8.

### 3.5 `Usage` mapping

`response.usage` on the completion event: `input_tokens`→`input_tokens`, `output_tokens`→`output_tokens`, `input_tokens_details.cached_tokens`→`cache_read_tokens` (`Some` iff present, else `None` — never `0`, architecture.md §3.2), no cache-write equivalent → `cache_write_tokens: None`. `total_tokens`/`output_tokens_details.reasoning_tokens` are derivable/long-tail → **dropped — discarded today** (canonical `Usage` holds only the four token fields; it has **no `provider_detail`** — that field lives on `CanonicalError`, not `Usage` — so there is nowhere for the detail to ride, and none is invented; §9 CR-Usage). Emitted **before** `Finish` (both ride the one `response.completed` frame; order within the returned `Vec` is `… ContentStop* → Usage → Finish`).

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

A non-2xx handshake arrives as a whole-body frame via the shared decoder contract (openai-chat-mapping.md §4.0) and is projected by the **one shared `json::http_error`** (bl-5fe6); the body is typically OpenAI's `{"error":{message,type,param,code}}` envelope, but the ChatGPT/codex backend returns a flat `{"detail":…}` — so `decode` emits `Event::Error{kind, message: <best-effort: error.message | detail | the body itself>, provider_detail: Some(<the WHOLE raw body>)}`, never assuming the `{"error":…}` shape. The HTTP status drives kind+exit per the shared table (architecture.md §8 — 400→`Provider{400}`/69, 401/403→`Auth`/77, 429→`Provider{429}`/69, 5xx→70). A mid-stream `response.error`/`response.failed` (after HTTP 200) is exited by its decoded `kind` via `from_kind`, status NOT consulted (architecture.md §8, CR-10): the error body's `code` (else `type`) tags the failure — `server_error` → `Provider{500}`/70, `rate_limit_exceeded`/`rate_limit_error` → `Provider{429}`/69, anything else (or no tag) → `Transport`/69. Never folded into `Finish` (architecture.md §3.3).

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

**The HeaderSpec proof (architecture.md §4.6).** Google authenticates with a custom header name (`x-goog-api-key`) carrying the raw key. This is already expressible as `HeaderSpec { name: "x-goog-api-key", scheme: Raw }` on the row. `ApiKeyAuth::apply` reads `auth.api_header` (data) and sets the named header to the raw secret (architecture.md §4.5, §7) — **no branch on "is this Google", no new `Auth` impl, no `AuthId` arm.** It is the identical mechanism that sets Anthropic's `x-api-key` (also `scheme: Raw`); only the `name` field of the data differs. Auth cost: **zero code, one field of one row.**

`framing(&self) -> Framing { Framing::Sse }` (with `?alt=sse`, Google emits SSE; the default JSON-array streaming form is not used). The model id is a **path segment** (`models/{model}:streamGenerateContent`), so `encode` builds the URL from `ctx.model` — a URL-shape difference absorbed entirely in `encode`, not a new seam.

### 4.2 REQUEST mapping — canonical → `generateContent` wire

| Canonical | Google wire | Rule |
|---|---|---|
| `model` | URL path `models/{ctx.model}` | alias-resolved; selects the endpoint, not a body field. |
| `system: Option<Vec<Content>>` | `"systemInstruction": {parts:[{text}]}` | text-only top-level. `None`/empty → omit. Non-`Text` → `Error{ParseInput}`/64 (architecture.md §3.1). |
| `messages` | `"contents"` (array of `{role, parts:[…]}`) | §4.3. Google roles are `"user"`/`"model"`. |
| `tools` | `"tools":[{functionDeclarations:[{name, description?, parameters}]}]` | **omit when empty.** `parameters` ← `input_schema` (an OpenAPI-subset schema `Value`, passed through). |
| `tool_choice` | `"toolConfig":{functionCallingConfig:{mode}}` | `Auto`→`AUTO` (or omit); `Any`→`ANY`; `None`→`NONE`; `Tool{name}`→`ANY`+`allowedFunctionNames:[name]`. |
| `parallel_tool_calls: Option<bool>` | — | **no wire field** — Google's `generateContent` has no parallel-tool-calls knob, so the lifted knob (architecture.md §3.1) is inexpressible and DROPPED. A genuine empty-set drop (the wire LACKS the field, unlike Responses which supports it, §3.2), not a silent narrowing of a supported field (§9 CR-R1). |
| `max_tokens` | `generationConfig.maxOutputTokens` | `Some`→value; `None`→omit. |
| `temperature`/`top_p` | `generationConfig.temperature`/`.topP` | `Some`→value; `None`→omit. |
| `reasoning: Option<ReasoningEffort>` | `generationConfig.thinkingConfig` | `Some(e)`→`{"thinkingBudget": e.budget(), "includeThoughts": true}` (the shared effort→budget table §6); `None`→omit. A `body_defaults` `thinkingConfig` object is the exact-budget escape hatch (rides `extra`; the typed knob wins). |
| `stop: Vec<String>` | `generationConfig.stopSequences` | **RENAME + nesting.** omit when empty. |
| `stream` | — | streaming is the **endpoint choice** (`:streamGenerateContent` vs `:generateContent`), not a body field — selected from `req.stream.unwrap_or(false)` in `encode`. No `stream` key on the wire. |
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
| `Content::Image{Url{url}}` | **rejects** → `Error{ParseInput}`/64 (§9 CR-G3). Gemini's `fileData.fileUri` references only files uploaded to the Google **Files API** (not arbitrary `https://…` web URLs, which it cannot fetch) and generally wants a `mimeType` sibling brazen cannot infer from a URL — no wire home, so a total reject (no prefix-sniffing), the image analogue of Ollama's base64-only slot (§5.4 CR-O2). Remedy named in the message: caller downloads and re-sends as `Base64`→`inlineData` (brazen never adds the round-trip, architecture.md §2) |
| `Content::Document{Base64{mt,data}}` | `{inlineData:{mimeType:mt, data}}` — **structured** base64 (`application/pdf` etc.), the same shape as an image. Documents are **input-only**; no provider returns one, so no decode side |
| `Content::Document{Url{url}}` | **rejects** → `Error{ParseInput}`/64 (§9 CR-G3, the SAME rule as `Image{Url}`): `fileData.fileUri` accepts no arbitrary web URL and wants an uninferrable `mimeType` — total reject, remedy named (re-send as a base64 document). One URL-slot rule across both media families |
| `Content::ToolUse{id,name,input,signature}` | `{functionCall:{name, args:input}}`, **plus a sibling `"thoughtSignature": signature`** when `signature` is `Some` (bl-61a9, resolving CR-G2) — `args` is a JSON **object** (not a string). The `thoughtSignature` is **LOAD-BEARING**: Gemini 2.5 multi-turn function calling **400s** if the `thoughtSignature` on a `functionCall` part is dropped on replay, so it is echoed back verbatim on the part that carries the call. **Google sends no tool-call id** → see §4.5 |
| `Content::ToolResult{tool_use_id, content, is_error}` | `{functionResponse:{name, response:{result:<text>}}}` — `name` is the function name resolved from the originating `ToolUse` via `tool_name(tool_use_id)`, falling back to the id only when that call is absent (keyed by **name**, not id — §4.5); the text result rides the `{result: …}` wrapper (Google's `response` is a free-form Struct that names `result` as an acceptable key — validated against the spec, bl-aba5); text-only-ish slot, non-`Text` → `Error{ParseInput}`/64. `is_error` surfaced textually |
| `Content::Thinking` / `RedactedThinking` | dropped — Google has no thinking-TEXT replay slot (the empty-set rule). The multi-turn signature that DOES matter is the `functionCall` `thoughtSignature`, which rides `ToolUse.signature` (above), not `Thinking` — it belongs to the tool call it accompanies (bl-61a9, §9 CR-G2 resolved). |

### 4.4 RESPONSE mapping — `streamGenerateContent` SSE → canonical `Vec<Event>`

Each SSE `data:` frame is a `GenerateContentResponse` chunk: `{candidates:[{content:{role:"model", parts:[…]}, finishReason?, index}], usageMetadata?}`. There is **no per-block start/stop on the wire** — the adapter synthesizes the canonical block structure.

| Wire feature (per chunk, `candidates[0]`) | Canonical events | DecodeState action |
|---|---|---|
| first chunk | `MessageStart{ id: None, model: Some(modelVersion?), role: Assistant }` once (gated `started`) | `started = true`. (Google streams no message id → `id: None`, never fabricated, architecture.md §3.2.) |
| first `parts[].text` | **synthesize** `ContentStart{i, Text {}}` then `ContentDelta{i, TextDelta(text)}` | open text block at `i = next_index++` |
| subsequent `parts[].text` | `ContentDelta{i, TextDelta(text)}` | — |
| `parts[].functionCall{name, args}` (arrives **whole**, not fragmented; a `parts[].thoughtSignature` sibling may accompany it) | **synthesize** `ContentStart{c, ToolUse{ id: synth, name }}`, then one `ContentDelta{c, JsonDelta(to_json_string(args))}`, then — when the part carries a `thoughtSignature` — `ContentDelta{c, SignatureDelta(thoughtSignature)}` (bl-61a9); the block stays open and **closes at the terminal drain** (the `finishReason` chunk below). A sink folds the `SignatureDelta` onto the tool block's `Content::ToolUse.signature` | assign canonical index (monotonic `open.len()`); id synthesized (§4.5) |
| `parts[].thought` text | `ContentStart{Thinking {}}`/`ThinkingDelta`/`ContentStop` (when `thinkingConfig` surfaces thoughts) | — |
| chunk carrying non-null `finishReason` (the **last chunk**) | drain open blocks → `ContentStop*`; then `Usage`(§4.6); then `Finish{reason}`(§4.7); then `[]` and **`state.terminated = true`** | the native terminator |
| `usageMetadata` on any chunk | `Usage` (§4.6) | cumulative |
| `promptFeedback.blockReason` / `finishReason=="SAFETY"` | `Finish{Refusal{category: blockReason\|"safety", explanation}}` (HTTP 200, exit 0 — architecture.md §3.2) | — |

**The terminator (architecture.md §3.4).** Google sends **no `[DONE]` and no `message_stop`** — the **last chunk carries a non-null `finishReason`**. `decode` recognizes that chunk as the terminal marker: it drains, emits `Usage`/`Finish`, and sets `state.terminated = true`. `decode` **never emits `End`**; `run` appends the one `End` at body EOF (architecture.md §4.4). A subsequent clean SSE EOF with `terminated` set suppresses the premature-EOF injection (architecture.md §5.6, CR-9). This is the one terminator whose marker is a *field on the content chunk* rather than a standalone sentinel frame — but the `terminated`-bit discipline is identical to every other protocol.

### 4.5 Tool-call id is synthesized (Google sends none); the result is keyed back by NAME

Google's `functionCall` carries **no id** (results are matched by function `name`). The canonical model requires `ContentStart{ToolUse{id, name}}` with a non-empty `id` so that identity-before-content holds and a folding consumer can key the call (architecture.md §3.2, §3.6). The adapter **synthesizes a deterministic id** — `"call_{candidateIndex}_{block_index}"` from `DecodeState` — so the canonical event shape is satisfied.

On the **request** side, Google keys a tool RESULT to its CALL by **function name** (`functionResponse.name`), not by id. The earlier conclusion — "`ToolResult.tool_use_id` is projected back to `functionResponse.name`" — was **wrong**: the harness replays the full transcript and sends `ToolResult{tool_use_id:"call_0_0"}`, so projecting the synthesized id straight onto `functionResponse.name` emits `"call_0_0"` where Google expects `"get_weather"` — an **illegal call** the model cannot associate with any call, silently breaking tool use. The correct projection **resolves the name**: the function name is a fact that lives once, on the originating `Content::ToolUse{id, name}`; the `ToolResult` references it by `tool_use_id`. brazen is stateless, but the originating `ToolUse` rides in the **same request** as the result, so the name is resolvable in-request with no state — a single shared query `CanonicalRequest::tool_name(tool_use_id) -> Option<&str>` (a query, NOT a copied field on `ToolResult`, which would denormalize and drift — SSOT) scans `Content::ToolUse` across the messages. `functionResponse.name` = `tool_name(id).unwrap_or(id)`: the resolved name, falling back to the id **only** when the originating call is genuinely absent (a bare tool-result turn) — the legitimate "fact not in-band" case, no fabrication. This is an adapter-owned projection, not a canonical change (architecture.md §3.1). See §9 CR-G1 (closed by this fix).

### 4.6 `Usage` mapping

`usageMetadata`: `promptTokenCount`→`input_tokens`, `candidatesTokenCount`→`output_tokens`, `cachedContentTokenCount`→`cache_read_tokens` (`Some` iff present), no cache-write → `cache_write_tokens: None`. `totalTokenCount`/`thoughtsTokenCount` derivable/long-tail → dropped. `Option` throughout — absent field is `None`, never `0` (architecture.md §3.2).

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

Non-2xx whole-body frame (shared decoder contract), projected by the **one shared `json::http_error`** (bl-5fe6); Google's envelope is `{"error":{"code":<int>,"message":"…","status":"…","details":[…]}}`. `decode` emits `Event::Error{kind, message: error.message (best-effort), provider_detail: Some(<the WHOLE raw body>)}`; the **HTTP status** drives kind+exit per the shared table (architecture.md §8). `error.status` (e.g. `RESOURCE_EXHAUSTED`, `PERMISSION_DENIED`) is informational, rides `provider_detail`. A mid-stream `{"error":{…}}` chunk on a 2xx SSE stream is exited by decoded `kind` (architecture.md §8, CR-10), same discipline as §3.7: the body's numeric `code` IS an HTTP-status int, so `kind` decodes from it through the one shared `from_http_status` table (the body's code, never the transport status a 2xx stream lacks). Without this path the chunk would be silently swallowed (no `candidates[]`).

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
auth = "none"                                             # keyless: no cred read, no auth header written
```

Local Ollama needs no auth, so the row is `auth = "none"` and carries **no `api_header`** — `NoAuth` reads no credential and writes no header (auth.md §3.3). `bz --provider ollama "hi"` works with **no `--api-key` and no `bz --login`**; a stray `--api-key` is accepted and ignored. (An operator pointing at a *gated remote* Ollama instead uses a keyed row — `auth = "bearer"` + an `api_header` — supplying a key via the normal cred path; no code difference.) Modeling keyless-local as `bearer`-with-a-tolerated-missing-key was rejected: it would silently downgrade a *forgotten* key on a real keyed provider from a clean 77 to a provider-side 401 (auth.md §3.3). `body_defaults` is **not** set — Ollama does not require `max_tokens` or any pinned body field.

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
| `tool_choice` | — | **no native field;** Ollama infers from `tools`. A required-tool intent rides `extra` if the model supports it (watch item, §9 CR-O1). |
| `parallel_tool_calls: Option<bool>` | — | **no wire field** — Ollama's `/api/chat` has no parallel-tool-calls knob (like `tool_choice` above); the lifted knob (architecture.md §3.1) is inexpressible and DROPPED. A genuine empty-set drop (the wire LACKS the field, unlike Responses §3.2), not a silent narrowing of a supported field (§9 CR-R1). |
| `max_tokens` | `options.num_predict` | **RENAME + nesting under `options`.** `Some`→value; `None`→omit. |
| `temperature`/`top_p` | `options.temperature`/`options.top_p` | nested under `options`; `None`→omit. |
| `reasoning: Option<ReasoningEffort>` | top-level `"think"` (bool) | `Some(_)`→`true` (any effort → think ON — Ollama's `think` is a plain bool with **no** effort granularity, so the three rungs collapse to "on"; §6); `None`→omit. NOT under `options`. A model that doesn't reason opts out via `unsupported_body_keys = ["reasoning"]` (config §4.1.1) — a 400-on-`think` model never sees the key. |
| `stop: Vec<String>` | `options.stop` | nested; omit when empty. |
| `stream` | `"stream"` | `req.stream.unwrap_or(false)`. `false` → a single NDJSON object (the folded stream, architecture.md §3.2). |
| `extra` | merged top-level / into `options` | the valve: `keep_alive`, `format` (JSON-mode/schema), `options.*` knobs (`num_ctx`, `seed`, `repeat_penalty`, …). Typed fields win (architecture.md §3.1). |

#### 5.4 `messages[]` projection

| Canonical `Content` | Ollama wire |
|---|---|
| `Text(s)` | message `content` string (concatenated for multi-text) |
| `Image{Base64{_,data}}` | `images:[<base64>]` — a **bare base64 array** on the message (Ollama drops the media-type; the raw base64 is the payload) |
| `Image{Url{..}}` | **UNREPRESENTABLE** — Ollama takes base64 only; a URL image → `Error{ParseInput}`/64 (a documented text/base64-only-slot rejection, architecture.md §3.1) |
| `Document{..}` (either source) | **UNREPRESENTABLE** — the Ollama chat wire has **no document slot at all** (only text + a bare base64 `images` array), so **both** document sources → `Error{ParseInput}`/64 (§9 CR-O3), the document analogue of the base64-only image rule. Message names the limitation, never a silent drop |
| `ToolUse{id?,name,input}` | assistant `tool_calls:[{function:{name, arguments:input}}]` — `arguments` an **object** (Ollama, unlike OpenAI, takes a JSON object, not a string). Ollama sends no tool-call id → synthesized on decode (§5.6) |
| `ToolResult{tool_use_id,content,is_error}` | `{role:"tool", content:<text>, tool_name?}` — Ollama's tool message carries an optional `tool_name`; emit the function name resolved via `tool_name(tool_use_id)` (§4.5, the same in-request query Google uses), omitting it only when the originating `ToolUse` is absent. Order still holds positionally; text-only, non-`Text` → `Error{ParseInput}`/64 |
| `Thinking{text,..}` | rides the assistant `thinking` field — `{role:"assistant", …, thinking:<text>}` (concatenated for multi-text; `signature` dropped — no Ollama slot; omitted when empty) so a `think` transcript round-trips losslessly (decode surfaces `message.thinking`, §5.5). Always emitted when present — encode cannot know model capability, and Ollama tolerates the field; *enabling* think OUTPUT rides `extra` (top-level `think`) |
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

The terminal `done:true` line carries token stats: `prompt_eval_count`→`input_tokens`, `eval_count`→`output_tokens`. Ollama reports **no cache fields** → `cache_read_tokens: None`, `cache_write_tokens: None` (never `0`, architecture.md §3.2). Durations (`total_duration`, `eval_duration`, …) have no canonical home → **dropped — discarded today** (`Usage` carries no `provider_detail` field; the detail has nowhere to ride and none is invented; §9 CR-Usage). `Option` throughout.

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

A non-2xx body is a single JSON object `{"error":"<message>"}` (a bare string, not OpenAI's nested envelope) — reached via the shared whole-body-frame decoder contract (the framer hands the whole non-2xx body as one frame regardless of `Ndjson` vs `Sse`, openai-chat-mapping.md §4.0) and projected by the **one shared `json::http_error`** (bl-5fe6). `decode` emits `Event::Error{kind, message: <the bare `error` string, best-effort>, provider_detail: Some(<the WHOLE raw body>)}`; the **HTTP status** drives kind+exit per the shared table (architecture.md §8). (The bare-string envelope is no longer special-cased: a non-JSON body keeps its status too, riding `provider_detail` as a `Value::String`.) Local Ollama failures are usually `Transport`/69 (connection refused — produced by the transport seam, not `decode`) or a `Provider{4xx}` for a bad request (model not pulled → 404 → `Provider{404}`/69). A mid-stream `{"error":"…"}` line on a 2xx stream is exited by decoded `kind` (CR-10, same discipline as §3.7/§4.8) — but the bare-string envelope carries no `type`/`code` discriminator, so the decoded kind is retryable `Transport`/69: the honest read of a kindless body, not an un-decoded default.

### 5.10 What's NOT touched (severability)

`run`, `resolve`, `parse`, `Sink`, the canonical model, the other Protocol impls, **and the NDJSON line-framer** (it already exists for the `--json` output `Sink` and is shared, architecture.md §5.2, §11) are unchanged. `framing() -> Ndjson` is a DATA return routed by `run`'s existing `framing.decoder()` switch (architecture.md §4.4) — `run` does not branch on the framing kind beyond that one data-driven `decoder()` call. `{"done":true}` normalizes to the same `Event::End`. Delete module + arm + insert → gone.

---

## 6. Reasoning effort (`--reasoning`) — one canonical knob, five wire shapes

`CanonicalRequest.reasoning: Option<ReasoningEffort>` (architecture.md §3.1) is a portable EFFORT intent (`low|medium|high`) the user sets once — via `--reasoning`, `BRAZEN_REASONING`, or a config-file `reasoning = "high"` — that each `encode` projects to its dialect's native reasoning shape. It is a **lifted known knob** (the third, after `ToolChoice` and `parallel_tool_calls`): every reasoning-capable provider names the same idea under an irreconcilable spelling, so routing the one intent to whichever dialect is in play is exactly the canonical→per-protocol mapping (`extra` could carry only one spelling). `--thinking` is the unrelated DISPLAY projection (architecture.md §5.3) — it never reaches the body.

**The effort→budget table is the single home for the budget dialects** (`ReasoningEffort::budget()`, architecture.md §3.1). Two dialects (Anthropic, Google) take a numeric thinking-token budget; both read the *same* table, so "how big is `medium`?" has one answer:

| Effort | `budget()` (thinking tokens) |
|---|---:|
| `low` | **1024** (the Anthropic `budget_tokens` minimum) |
| `medium` | **8192** |
| `high` | **24576** |

**Per-protocol projection** (each owned by that protocol's `encode`; `e.as_str()` = `"low"`/`"medium"`/`"high"`, `e.budget()` = the table above):

| Protocol | Wire shape for `Some(e)` | Spec home |
|---|---|---|
| `openai_responses` | `"reasoning": {"effort": e.as_str()}` | §3.2 |
| `openai_chat` | `"reasoning_effort": e.as_str()` | openai-chat-mapping.md §2 (this row is the canonical map) |
| `anthropic_messages` | `"thinking": {"type":"enabled", "budget_tokens": e.budget()}` + the `max_tokens` coupling below | anthropic-messages.md §2 (this row is the canonical map) |
| `google_generative_ai` | `generationConfig.thinkingConfig = {"thinkingBudget": e.budget(), "includeThoughts": true}` | §4.2 |
| `ollama_chat` | top-level `"think": true` (any effort → on; **no** effort granularity) | §5.3 |

`None` omits the key entirely on every protocol (the empty-set path, not a special case).

**The Anthropic `max_tokens` / `budget_tokens` coupling (the one constraint the encoder must guarantee).** Anthropic's extended-thinking API requires `budget_tokens >= 1024` (satisfied by the table — `low` IS 1024) **and** `max_tokens > budget_tokens` (the thinking budget is carved OUT of `max_tokens`, so the request needs room for thinking *and* an answer). `anthropic_messages` already requires `max_tokens` (a `None` is a Config error, §3.2), so at encode it is always present; when `reasoning` is set the encoder enforces the floor:

```
budget       = e.budget()
effective_max_tokens = max(req.max_tokens, budget + REASONING_HEADROOM)   // REASONING_HEADROOM = 4096
```

So `max_tokens` is bumped to `budget + 4096` whenever the caller's value is below that floor (the default 4096 with `--reasoning high` → 28672), and a caller who set a generous `--max-tokens` keeps it. This **guarantees** `max_tokens > budget_tokens` with a 4096-token answer allowance, and never errors — brazen normalizes to what the provider accepts (the same silent-normalization discipline as the always-stream force and `strip_unsupported`, config §4.1.1). **Additionally, `temperature`/`top_p` are OMITTED on the Anthropic wire when `reasoning` is set** — extended thinking only accepts `temperature: 1` (and restricts `top_p`), so emitting the caller's sampling params would 400; the per-dialect projection drops them (they are untouched on the canonical request, available to every other protocol). The decision: *bump, don't error* — a reasoning request with too small a `max_tokens` is physically impossible to satisfy any other way, and erroring would punish the common case (`--reasoning high` with the row's default `max_tokens`). The headroom and budget constants live in the encoder (Anthropic-dialect data), the budget table on the shared enum.

**Escape hatch & opt-out (single-source, severable).** The three-rung enum is deliberately coarse. An exact budget, an adaptive `{type:"adaptive"}` object, or a per-effort override is reached via the routed row's `body_defaults` (config §4.1) — a provider-shaped reasoning object pinned there (`thinking = {...}`, `thinkingConfig = {...}`, `reasoning = {...}`) rides `req.extra` to the wire verbatim, and the typed `--reasoning` knob, written by `encode` *before* the `extra` fold, WINS on a same-named key (so the two never silently combine). A backend that rejects reasoning lists the canonical key `reasoning` in `unsupported_body_keys`; `strip_unsupported` clears it pre-encode (config §4.1.1), so e.g. a non-reasoning Mistral chat model never emits `reasoning_effort`. Deleting the row datum deletes the behavior — no core branch on "does this model reason."

### 6.1 Structured output (`req.output`) — one canonical knob, five wire shapes

`CanonicalRequest.output: Option<OutputFormat>` (architecture.md §3.1) is a portable structured-output intent — `Json` (schemaless JSON mode) or `JsonSchema{name, schema, strict}` — that each `encode` projects to its dialect's native structured-output shape. It is the **fourth lifted known knob** (after `ToolChoice`, `parallel_tool_calls`, `reasoning`): every structured-output-capable dialect names the same idea under an irreconcilable spelling, and two of them (Google, Ollama) *nest* or *rename* it, so `extra` — a flat top-level valve carrying one spelling — cannot express it portably. `None` = plain text (the empty-set path, not a special case).

**Per-protocol projection** (each owned by that protocol's `encode`; the typed knob is written BEFORE the `extra` fold, so it WINS on a same-named `body_defaults`/`extra` key):

| Protocol | `Json` (schemaless) | `JsonSchema{name, schema, strict}` | Spec home |
|---|---|---|---|
| `openai_chat` | `response_format: {"type":"json_object"}` | `response_format: {"type":"json_schema","json_schema":{name, schema, strict?}}` (name defaults `"response"`) | openai-chat-mapping.md §2.5.1 |
| `openai_responses` | `text: {"format":{"type":"json_object"}}` | `text: {"format":{"type":"json_schema","name","schema","strict"?}}` — **FLAT under `format`**, no `json_schema` wrapper (the one shape that differs from Chat) | §3.2 |
| `google_generative_ai` | `generationConfig.responseMimeType: "application/json"` | `generationConfig.{responseMimeType:"application/json", responseSchema: <schema>}` — `name`/`strict` **narrowed** (no field) | §4.2 |
| `ollama_chat` | top-level `format: "json"` | top-level `format: <schema object>` — `name`/`strict` **narrowed** | §5.3 |
| `anthropic_messages` | **OMITTED** — Anthropic has no schemaless JSON mode → documented narrowing (like `stop` on Responses, CR-R1) | `output_config: {"format":{"type":"json_schema","schema"}}` (GA, no beta header; SCHEMA-ONLY — `name`/`strict` **narrowed**) | anthropic-messages.md §2.12 |

`None` omits the key on every protocol. The escape hatch for a value the enum can't express (a raw `response_format`, a `responseSchema` with a Google-specific `propertyOrdering`, an Anthropic `output_config` object) stays the row's `body_defaults` (config §4.1), pinned there and riding `req.extra` verbatim; the typed `output` knob, written *before* the fold, wins on a same-named key. A backend that rejects structured output lists the canonical key `output` in `unsupported_body_keys`; `strip_unsupported` clears it pre-encode (config §4.1.1).

**`Tool::Custom.strict` — the per-tool sibling** (architecture.md §3.1): OpenAI-style strict function calling, lifted the same way because it too nests inside the per-tool object `extra` cannot reach. Projected as `function.strict` (`openai_chat`), FLAT `strict` on the tool (`openai_responses`, `anthropic_messages`); Google `functionDeclarations` and Ollama function objects LACK the field → **narrowed** (dropped). `None` omits it, byte-stable with the pre-knob `Custom` wire. Before lifting, a wire `strict` on a custom tool was silently dropped by `Custom`'s decode.

---

## 7. Prompt caching — automatic everywhere, zero canonical surface

Prompt caching has **no canonical surface**: no request field, no flag, no config key (architecture.md §2/§3.1). Every dialect caches automatically; the only cross-provider difference is WHO places the marker, and that difference is adapter-internal:

- **`anthropic_messages`** — the one dialect whose wire demands explicit per-block `cache_control` markers. Its encoder places them **automatically from the request's shape** (the full policy is anthropic-messages.md §2.10): a head mark always (last `system` block, else last `tools` object, else nothing); a rolling mark on the last eligible block of the last non-assistant wire message when the request is an ongoing conversation (at least one assistant turn strictly before the last message — a lone trailing-assistant prefill never triggers); one intermediate mark 20 eligible blocks behind the rolling mark on a long span. ≤3 marks by construction (the provider's 4-cap is unreachable — no error path); `thinking`/`redacted_thinking` blocks are ineligible and step the mark back; TTL is always omitted (the renewing 5m default — `1h` only wins across idle gaps a stateless adapter cannot see).
- **OpenAI Responses/Chat, Google, Ollama** — cache automatically by prompt PREFIX on the provider side. There is no marker concept, nothing is declared, and nothing is dropped **because nothing is declared** — zero code.

The marks are written BEFORE the `extra` fold, so a policy `cache_control` WINS over a raw one an `extra` key carries (anthropic-messages.md §2.1.1). The escape from the policy (e.g. a non-recurring batch replay that must not pay the one-time 25% cache-write premium) is `--raw` — provider-native bytes, no placement; a typed opt-out is additive later if real usage demonstrates the need.

**Cache placement vs response-cache-tokens are NOT the same fact.** Placement is a REQUEST-side act the adapter performs invisibly; the response-side `Usage.cache_read_tokens`/`cache_write_tokens` (§3.5 OpenAI, §4.6 Google, §5.7 Ollama, and the Anthropic Usage mapping) report cache HITS/WRITES that ALREADY happened — the caller's one window onto the policy's effect. The two never conflate: a provider that auto-caches by prefix still reports `cache_read_tokens` without any marker ever existing.

---

## 8. The severability ledger (the executable grading rubric)

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

## 9. Edge cases & architecture change requests

Per the derivation rule (architecture.md §1 of each mapping spec): nothing is silently deviated. Each gap is resolved here, resolved-in-architecture, or **deferred** as a genuine open item. The shared, already-resolved-in-architecture items (`extra` precedence with typed fields winning; the non-text-slot `ParseInput`/64 rejection; externally-tagged `ContentKind`/`Delta` serde; `DecodeState.terminated`/premature-EOF; post-200 mid-stream exit-by-`kind`) apply to **every** protocol here exactly as in the sibling mappings — not re-litigated.

### Resolved here (no canonical change)

- **Mistral wire deviations** (§2.3): every one fits in `extra` / an empty-set path / a row signal. Zero code. The severability proof.
- **Synthesized tool-call ids for Google & Ollama** (§4.5, §5.6): both wires send no id; the adapter synthesizes a deterministic id to satisfy `ContentStart{ToolUse{id,name}}` (architecture.md §3.2), the same synthesis pattern OpenAI chat already uses for its `ContentStart`. On re-send, `ToolResult` is keyed back by `name`/position. Adapter-owned projection, no canonical change.
- **Whole (non-fragmented) tool args for Google & Ollama** (§4.4, §5.6): emitted as a **single** `JsonDelta`, the block then closing at the terminal drain (so `open.len()` stays a monotonic, never-stored canonical index — the same drain discipline OpenAI uses); the "valid only concatenated" rule (architecture.md §3.6) holds trivially for one fragment.
- **Field-on-content-chunk terminators** (Google `finishReason`, Ollama `done:true`): unlike the standalone sentinels (`[DONE]`/`message_stop`/`response.completed`), the marker rides the final *content* line. The adapter emits that line's content events first, then drains + finishes + sets `terminated`. The `terminated`-bit discipline (architecture.md §5.6, CR-9) is identical regardless of marker shape. No change.
- **`Framing::Ndjson` for Ollama** (§5.2): `Framing` is DATA (architecture.md §4.1); the NDJSON line-framer already exists (the `--json` `Sink`, architecture.md §5.2). One data return value, no new framer, no `run` branch. No change.
- **`x-goog-api-key` as `HeaderSpec` data** (§4.1): expressible today (architecture.md §4.6); `ApiKeyAuth` reads `auth.api_header`. No new `Auth`, no change.

### Deferred / watch items (genuine gaps — recorded, not silently worked around)

- **CR-R1 — the narrowing principle, and the `parallel_tool_calls` bug it caught (FIXED).** The rule: dropping a typed field on encode is a legitimate **narrowing only when the wire LACKS the field** (like `Thinking` on Chat re-send); silently dropping a typed field the wire **DOES support** is a **bug**. Two cases here:
  - **`stop` — a genuine narrowing (deferred).** The Responses API has no top-level `stop` field (§3.2), so a non-empty `stop` is omitted unless the caller supplies the wire key via `extra`. Documented, not a canonical change. Raised only if `stop` on Responses becomes load-bearing. **Low urgency.**
  - **`parallel_tool_calls` — was a bug, now FIXED (bl-a9e2).** The Responses wire **supports** top-level `parallel_tool_calls`, yet the encoder emitted nothing — a silent drop of a supported typed field, exactly what this principle forbids. It is now emitted top-level like Chat Completions (§3.2, openai-chat-mapping.md §2.6). The Google/Ollama drops of the same knob ARE legitimate narrowings (those wires lack the field, §4.2/§5.3).
- **CR-R2 / G-equiv / O-equiv — `ToolResult.is_error` has no native field** on Responses (`function_call_output`), Google (`functionResponse`), or Ollama (`tool` message). Surfaced **textually** (content prefix), identical to the OpenAI chat degradation (openai-chat-mapping.md §6 CR-C3). The structured boolean does not round-trip. Same deferred CR as the OpenAI mapping — one resolution covers all dialects.
- **CR-R4 — reasoning wire shape VERIFIED; both reasoning channels → Thinking; channel coexistence ruled out (bl-410e, bl-7e50, bl-d884).** Verified against OpenAI's published Responses streaming reference (the authoritative wire contract; no live capture taken — no key in env, and a live capture only re-confirms published field names, not the representation decision): `response.reasoning_summary_text.delta` carries `{type, item_id, output_index, summary_index, delta, sequence_number}` — **`summary_index`, NO `content_index`** — confirming the §3.4 assumption that summary deltas route to pair `(output_index, 0)` (the Thinking block opened on the `reasoning` item add). `part_key()` reads `content_index` (absent → `0`) for these events, correct *only because* the summary channel never sends `content_index`. **Per-part decision: keep the one-block collapse** (all `summary_index` parts concatenate into the single canonical `(output_index, 0)` Thinking block) — `ContentKind::Thinking {}` has no part slot, matching Anthropic's single-thinking-block model, and the collapse drops no data. The symmetric per-part path is confirmed real (a sibling `response.reasoning_summary_part.added` event exists with `summary_index`, exactly paralleling `content_part.added`'s `content_index`); building it is deferred to a real consumer need + a capture of how many parts models actually emit (the count does not affect the collapse's losslessness). **Second channel — HANDLED (minimal, bl-7e50):** a distinct `response.reasoning_text.{delta,done}` family streams *raw* (non-summary) reasoning from the item's `content[]` and carries a `content_index` (verified-org / `include`-gated, typically not user-visible). `reasoning_text.delta` now routes as a `ThinkingDelta` by pair `(output_index, content_index)`; for `content_index 0` it lands in the Thinking block the `reasoning` item-add already opened (no new open logic; `.done` is a no-op, closed by `output_item.done`). **Two documented caveats:** (i) *coexistence — does NOT occur on the hosted Responses API (bl-d884, RESOLVED).* The two channels are gated by **disjoint model classes**, so a single hosted Responses SSE stream emits one or the other, never both: hosted reasoning models (gpt-5 / o-series) emit the **summary channel only** — raw CoT is hidden by design ("OpenAI doesn't expose the chain of thought for GPT-5-Thinking, and no `reasoning_text` appears in the response"), which is the *stated purpose* of the Responses API — while the raw `reasoning_text` channel is a **gpt-oss** (open-weight) feature, for which OpenAI's own cookbook directs developers to run their *own* summarizer over the raw CoT (the summary is a downstream developer step, not a second channel the hosted response emits). So the feared interleave into pair `(output_index, 0)` cannot arise on the hosted API, and the bl-7e50 raw route stays a clean addition. It remains only *schema-theoretically* possible (a `reasoning` item's JSON carries both a `summary[]` and a `content[]` array), so the coexistence rule (raw-wins / summary-wins, or distinct per-part blocks via the §3.4 per-part seam) stays **unbuilt under YAGNI** — were both arrays ever populated in one stream, the current code interleaves their deltas in wire-arrival order (lossless bytes, possibly unreadable), which is the trigger to build the rule. *Basis:* determined from OpenAI's published Responses streaming reference + the reasoning / gpt-oss guides — the same authoritative-contract basis as the bl-410e wire-shape verification; no live capture taken (no OpenAI key in env, bl-d884), and a capture would only confirm the negative empirically, not change the model-class gating that establishes it. (ii) *multi-part raw* — `content_index > 0` routes to an unopened pair and is dropped, the same limitation multi-part summary has (no per-part Thinking block; see the per-part decision above; the summary-part-count capture that decision defers was likewise not obtainable without a key, and the count does not affect the collapse's losslessness). **Low urgency.**
- **CR-O1 — Ollama has no `tool_choice` field** (§5.3): a required-tool intent (`ToolChoice::Any`/`Tool{name}`) has no native Ollama spelling; v0.1 omits it (Ollama infers from `tools`). Raised only if forcing a tool call on Ollama becomes load-bearing. **Low urgency.**
- **CR-O2 — Ollama URL-image rejection** (§5.4): Ollama takes base64 only, so `Image{Url}` → `Error{ParseInput}`/64 — a documented base64-only-slot rejection (architecture.md §3.1), the image analogue of the text-only-slot rule. No change.
- **CR-O3 — Ollama document rejection** (§5.4): Ollama's chat wire has **no document slot at all** (only text + a bare base64 `images` array), so a `Content::Document` in ANY message → `Error{ParseInput}`/64 — the document analogue of the base64-only image rule (CR-O2), a documented total-slot rejection (architecture.md §3.1), never a silent drop. No change.
- **CR-Doc — canonical documents (`Content::Document{DocumentSource}`) map/narrow per dialect** (architecture.md §3.1, bl-956c): the `Image` analogue for PDFs/files, INPUT-ONLY. `DocumentSource::{Base64{media_type,data}|Url{url}}` mirrors `ImageSource`. Per-dialect: Anthropic maps **both** sources (`{type:document, source:{...}}`, anthropic-messages.md §2.5); OpenAI **Responses** maps **both** (`input_file`: base64 `file_data`, `url`→`file_url`, §3.3); OpenAI **Chat** maps base64 (`{type:file,file:{file_data,filename}}`) but **rejects the URL** (chat file inputs take no web URL — openai-chat-mapping.md §6 CR-C6); **Google** maps base64→`inlineData` and **rejects the URL** (CR-G3, same rule as `Image{Url}`); **Ollama rejects both** (CR-O3). Every reject is at encode with `Error{ParseInput}`/64, documented, never silent (architecture.md §3.1). **No decode side** — no provider returns a `document` block (documents are input-only). **Boundary held:** the ball added ONE variant + one source enum, additive to the request parse (old requests parse unchanged); it did **NOT** add a per-block `extra` valve (a bigger door — noted for the spec, deferred, architecture.md §3.1). AUDIO deferred with rationale (architecture.md §3.1, CR-Audio).
- **Server tools (architecture.md CR-4, resolved there) — the four dialects here take the documented DEGRADATION, not the capability.** `Tool::Provider{kind,name,config}` and `Content::ServerToolUse`/`ServerToolResult` (architecture.md §3.1) are opaque Anthropic-carried passthrough; none of the encoders in this spec projects them. A `Tool::Provider` in `tools[]` → **encode-time `Error{ParseInput}`** (exit 64, "provider-typed tools are not projected for this dialect") on OpenAI chat, OpenAI Responses, Google, and Ollama — fail fast, never a silent drop. A `Content::ServerTool*` block in a replayed transcript → `ParseInput`/64 on OpenAI chat/Responses/Ollama (the existing non-representable-content rejection), and is **dropped** by Google (folded into the `Thinking`/`RedactedThinking` empty-set drop, §4.3). Server-tool RESULT blocks are never decoded/surfaced by these dialects (no such wire block exists — the empty-set rule). **Future per-dialect work:** the OpenAI **Responses** API has native typed tools (`web_search`, `code_interpreter`, …); projecting `Tool::Provider` onto them is a later ball. `Tool::Custom` is unaffected everywhere.
- **CR-Usage — dropped `Usage` detail is DISCARDED today; no `provider_detail`-on-`Usage` field is promised until a consumer exists.** Several dialects report token/duration detail the canonical `Usage` has no slot for: OpenAI Responses `output_tokens_details.reasoning_tokens`/`total_tokens` (§3.5) and Ollama `total_duration`/`eval_duration`/… (§5.7). Canonical `Usage` (architecture.md §3.2) carries only `input_tokens`/`output_tokens`/`cache_read_tokens`/`cache_write_tokens` — and **no `provider_detail` field** (that field lives on `CanonicalError`, not `Usage`). Earlier text said this detail "rides `provider_detail`"; that described an **UNBUILT mechanism as if decided** — the kin of "don't store what you can compute": *don't promise what you haven't built*. The honest state: the detail is **dropped, not stashed anywhere**. **Deferred:** no `Usage` field (and no `provider_detail` on `Usage`) is added until a real consumer needs one, at which point it is an additive `v=1` change (architecture.md §3.2) — the same shape in which the server-tool usage counter is scoped (architecture.md §3.3). **Low urgency.**

### Resolved (formerly filed under Deferred / watch — text unchanged, only the bucket corrected)

These carry an inline **RESOLVED**/**CLOSED** label; they are kept for provenance but no longer misfiled under a watch heading.

- **CR-R3 — Responses reasoning replay: RESOLVED via encrypted reasoning items (bl-61a9; owner ruling 2026-07-08 supersedes the "low urgency"/"out of scope for v0.1" deferral).** The stateless (`store:false`) path — which the codex/ChatGPT-SSO row mandates — is now first-class: `encode` requests `include:["reasoning.encrypted_content"]` whenever `req.reasoning` is set (§3.2), `decode` captures the reasoning item `id` at open and the `encrypted_content` at `output_item.done` (§3.4), and `encode` reconstructs a `{type:"reasoning", id?, summary?, encrypted_content}` input item from `Content::Thinking{id, encrypted_content}` when `encrypted_content` is `Some` (§3.3). `previous_response_id` (the stateful alternative) stays a caller `extra` key — brazen is stateless and does not track response ids. A `Content::Thinking` with no `encrypted_content` is still dropped (nothing to replay statelessly), consistent with the empty-set rule.
- **CR-G1 — Google tool-call id absence** (§4.5): **CLOSED** (bl-dd9b). The watch item's original conclusion ("no architecture change required") was wrong on the *request* side: projecting the synthesized id straight onto `functionResponse.name` produced an illegal call (the name-keyed dialect cannot match `"call_0_0"` to a function). Resolved without a canonical change by one shared in-request query, `CanonicalRequest::tool_name(tool_use_id)`, that both NAME-keyed dialects (Google `functionResponse.name`, Ollama tool-message `tool_name`) consume — the function name stays a single fact on the originating `ToolUse` (SSOT), resolved per-request, never copied onto `ToolResult`. The DECODE-side synthesized id remains a sound adapter projection (matches OpenAI's synthesized-`ContentStart`).
- **CR-G2 — Google thought signatures: RESOLVED (bl-61a9; owner ruling 2026-07-08 supersedes the "low urgency" deferral).** Wire truth (verified against Google's Gemini thought-signatures docs): `thoughtSignature` is a base64 string sibling of a part's `functionCall`, and Gemini 2.5 multi-turn function calling **400s** if it is dropped when echoing the call back. It is therefore normalized onto **`Content::ToolUse.signature`** (NOT `Thinking.signature` — it belongs to the tool call it accompanies): `decode` surfaces it as a `Delta::SignatureDelta` on the tool block (§4.4), a sink folds it onto `ToolUse.signature`, and `encode` re-emits it as the `functionCall` part's `thoughtSignature` sibling (§4.3). Google's thinking-TEXT is still dropped (no replay slot; empty-set rule). Thought signatures on non-`functionCall` parts are out of scope — Google's thinking-text is not replayed, so those have no round-trip home.
- **CR-G3 — Google `Image{Url}` rejection** (§4.3): **RESOLVED (bl-97a7) — total reject, decision (a).** The prior encode mapped `Image{Url{url}}` → `{fileData:{fileUri:url}}`, implying support it did not have: Gemini's `fileData.fileUri` accepts **only Google-hosted file references** — a Files-API resource (`https://generativelanguage.googleapis.com/v1beta/files/…`) or, on Vertex, a GCS `gs://` URI — and **cannot fetch an ordinary `https://…png`** (unlike OpenAI chat/responses, which pass web URLs through natively). So a canonical `Image{Url}` carrying a normal web URL failed at the provider with a confusing 400 while the encode implied it would work — the §3.1 mistranslation-not-rejection anti-pattern. Now `Image{Url}` **rejects at encode with `Error{ParseInput}`/64** (architecture.md §3.1: a wire slot that cannot express the content rejects — a documented degradation, never a silent mistranslation), message naming the limitation and the remedy (download and re-send as `Base64`→`inlineData`; brazen never adds the round-trip, architecture.md §2). **(a) total vs (b) pass-through Google-file/GCS prefixes, weighed:** (b)'s only genuine beneficiary is a caller who legitimately holds a Files-API URI and wants brazen's normalization *and* a `fileData` reference — but three facts sink it. **(i) mimeType.** `fileData` generally wants a `mimeType` sibling, and brazen **cannot infer one from `Image{Url{url}}` alone**; a Files-API URI can omit it (the server knows the uploaded file's type), but a `gs://` URI on Vertex **requires** it — so a prefix pass-through of `gs://` would emit a `fileData` the provider *still* 400s for a missing `mimeType`, reproducing the very confusing-400 this ball removes. To be honest, (b) would have to narrow to Files-API URIs only and reject `gs://` too — a stranger, more surprising rule than a clean total reject. **(ii) prefix-sniffing is a smell** — branching encode behavior on the *string content* of a URL couples the Google adapter to Google's URL namespace (which can change) and is exactly the structural sniffing the architecture forbids (architecture.md §2, "no structural sniffing"). **(iii) `Image{Url}` means "a web image", not "a provider file handle"** — a Files-API URI stuffed into it is already provider-specific, so the sanctioned escape is `--raw` (provider-native bytes, architecture.md §5.4/§2), not a canonical-field overload. So there is **no accepted URL form**: the message names the limitation, not a whitelist. Sibling of **CR-O2** (Ollama base64-only URL rejection) — one image-slot rule across the base64-/file-only dialects, no per-dialect special case. **No canonical change** (adapter-owned projection). **The SAME rule now covers `Document{Url}`** (bl-956c, §9 CR-Doc): Gemini's `fileData.fileUri` accepts no arbitrary web URL for a document either, so `Document{Url}` → `Error{ParseInput}`/64 with the identical remedy (re-send as a base64 document → `inlineData`) — one URL-slot rule across BOTH media families, no new mechanism.

### Cross-spec consistency (relied on, not a change)

- **All four protocols here uphold the same invariants the sibling mappings pin** (§1.1): typed fields win over `extra`; provider 4xx→69 / 5xx→70 / 401-403→77 (architecture.md §8); `decode` never emits `End` and sets `terminated` on the terminal marker; `Usage` fields `Option` (never fabricated `0`); refusal is `Finish{Refusal}` at exit 0. These are the consistencies that let any future cross-protocol equality test (architecture.md §3.6) be writable — not architecture changes.

---

## 10. Models-listing endpoints

Each of these rows also serves `bz --list-models` via its protocol's `models_shape` (the per-dialect path + list-key DATA) fed to the one generic `decode_models` — a GET to a per-dialect endpoint whose body projects onto `Vec<Model>` and is written to the per-provider cache the generation path reads (model-discovery.md §5). Those per-protocol facts (paths, list shapes, the Google `models/` strip) live in **one home**, [model-discovery.md §3.1](model-discovery.md), so they are not duplicated here. The capability adds no new `Auth` (the verb's GET reuses `Auth::apply`); a standard row needs no per-row field, while a row whose discovery endpoint diverges from its protocol's default (the ChatGPT-SSO Codex backend) pins the optional, severable `[provider.models]` override (path/query/array_key/id_key, model-discovery.md §3.2).

---

## 10.1 Token-counting endpoints — the `--count-tokens` control op (bl-24e5)

The `--count-tokens` control op (architecture.md §5.10.1) does ONE round-trip to a provider's token-count endpoint and returns a **provider-accurate** input-token count for a canonical request (read the SAME way the data plane reads one). Endpoint knowledge is DATA on the protocol: `Protocol::count_tokens(req, ctx) -> Option<Result<CountRequest, _>>`, the sibling of `models_shape()`/`path()`. It **defaults to `None` — the decline** — so a dialect opts IN only by overriding; a dialect with no count endpoint needs zero code. `CountRequest` carries the POST `WireRequest` (count-endpoint URL + count body built from THIS dialect's own `encode` projection) and the response's token-count JSON key. The count runner stamps `content_type`/beta headers/timeouts/auth (as `serve` does), sends once, drains, and reads the key. **No retry, no cache write.**

- **Anthropic (`anthropic_messages`) — LIVE.** `POST /v1/messages/count_tokens`, body = the §2 messages/system/tools body minus generation-only keys; response key `input_tokens`. Full mapping: [anthropic-messages.md §2.11](anthropic-messages.md).
- **Google generative-ai (`google_generative_ai`) — LIVE.** `POST {base_url}/v1beta/models/{model}:countTokens`; response `{"totalTokens": N}` (key `totalTokens`). Google's `countTokens` accepts **either** a bare `contents[]` (which would undercount — it omits `systemInstruction`/`tools`) **or** a `generateContentRequest` envelope wrapping a full `GenerateContentRequest`. To count the whole request faithfully, `count` reuses this dialect's `encode` body-assembly (`body_map` — the shared `systemInstruction`/`contents`/`tools`/`toolConfig`/`generationConfig`/`extra` projection, §4.2, extracted so `encode`'s own bytes are unchanged), injects the required `model` (`models/{model}`, which the URL path omits), and wraps it as `{"generateContentRequest": {…}}`. `generationConfig` (a valid `GenerateContentRequest` member) is left in — it does not affect the input-token total. This is the one per-dialect count asymmetry (Anthropic drops keys; Google wraps + injects `model`), and it lives entirely in Google's `count`, behind the shared seam — no new trait, config, or data flow, so it is a clean fold, not new mechanism.
- **OpenAI chat (`openai_chat`), OpenAI responses (`openai_responses`), Ollama (`ollama_chat`) — DECLINE.** None has a first-party count endpoint; each keeps the trait default (`None`), so `--count-tokens` against them is the honest `Config` (78) decline (architecture.md §5.10.1/§8). The caller's own estimate stays its fallback — a fabricated count is a lie (Usage-Option-not-zero, architecture.md §3.2). If a future backend gains a count endpoint, it opts in by overriding `count_tokens` — a per-row/per-dialect addition, zero core change.

---

## 11. Summary of decisions (this spec is decisive)

- **Mistral** = one `[[provider]]` row on `protocol="openai_chat"`+`auth="bearer"`, **zero Rust**; deletes cleanly; every wire deviation fits in `extra`/empty-path/row data. The severability floor.
- **OpenAI responses** = `mod openai_responses` + `ProtocolId::OpenAiResponses` arm + one insert + a row. `Framing::Sse`. `system`→`instructions`, `messages`→`input[]`, `max_tokens`→`max_output_tokens`; `response.completed`→`Usage`+`Finish`+`terminated`; `run` appends the one `End`.
- **Google generative-ai** = `mod google_genai` + one arm + one insert + a row whose `api_header = { name="x-goog-api-key", scheme="raw" }` is the **HeaderSpec-is-data proof** (no new `Auth`). `Framing::Sse`. Model in the URL path; roles `user`/`model`; structured `inlineData` images; **last chunk's non-null `finishReason`** is the terminator → `terminated`.
- **Ollama** = `mod ollama_chat` + one arm + one insert + a row. **`framing() -> Framing::Ndjson`** (the distinctive cost; the line-framer is shared, not redefined). Params nested under `options`; tool args & ids synthesized whole; **`{"done":true}`** is the terminator → `terminated`.
- **One terminator for all.** `response.completed` / `finishReason`-bearing-last-chunk / `{"done":true}` all normalize to the **same single `Event::End`** appended once by `run`; `decode` never emits it; each sets the same `terminated` bit gating the same premature-EOF injection (architecture.md §3.4, §4.4, §5.6).
- **No core change for any addition.** `run`/`resolve`/`parse`/`Sink`/canonical model/other Protocol impls are untouched; dispatch is by `ProtocolId`/`AuthId` map keys, never a vendor name (architecture.md §4.4, §4.6).

CITATIONS: https://platform.openai.com/docs/api-reference/responses · https://platform.openai.com/docs/api-reference/responses-streaming · https://ai.google.dev/api/generate-content · https://ai.google.dev/gemini-api/docs/text-generation · https://github.com/ollama/ollama/blob/main/docs/api.md · https://docs.mistral.ai/api/
