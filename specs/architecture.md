# Architecture & I/O Contract

> **Living document.** Edited like code. Per-protocol/-provider/-auth specs derive from this one and must not contradict it; if they need to, this spec changes first.

---

## 1. Purpose & Scope

`brazen` (binary `bz`) is **one small, stateless Rust binary** that adapts every LLM provider (OpenAI, Anthropic, Mistral, Google, local Ollama, …) and every wire protocol (OpenAI `chat/completions`, OpenAI `responses`, Anthropic `messages`, Google `generative-ai`) behind a single pipe contract:

```
stdin (canonical request) → bz → stdout (canonical event stream, streamed until one End token)
```

It is a **low-level building block for agents**, not an agent. Its **generation data plane does exactly one network round-trip per process**, normalizes the provider's stream into one canonical event vocabulary, and exits with a POSIX-correct code. Two qualifications, neither agentic: (a) two **control operations** — `bz --login` (auth, §7) and `bz --list-models` (one GET, model-discovery.md) — are distinct from the data plane (control-short-circuit flags, not verbs — §5.10.1, §13.13); (b) a generation request resolves its model against a **per-provider model cache** (a local file read, offline — written *only* by `bz list-models`, never by the data plane) before its one generation round-trip, falling back to attempting the model string **verbatim** on a cache miss. **brazen never lists automatically** — the generation path never makes a model-list GET, never retries, never spawns (§2); a cold or stale cache is the caller's to refresh with `bz list-models`. Every generation is one round-trip. It handles all auth models (API key, bearer, OAuth/SSO with browser launch). It is published as a crate so the pure pipeline can be embedded directly.

This spec is the authoritative **architecture and I/O contract**: the spine, the canonical model, the adapter abstraction, the I/O/streaming/POSIX behavior, config/credentials/auth, the error model, and the testability/portability constraints. It is decisive: where a choice exists, this document makes it.

### The spine (the whole binary in one signature)

```rust
fn run(
    args:      Args,            // injected argv + env snapshot + stdin-isatty (the lib never reads std::env or probes the tty)
    stdin:     &mut dyn Read,
    stdout:    &mut dyn Write,
    stderr:    &mut dyn Write,  // pre-sink fatals + --text in-band Event::Error (§5.9)
    transport: &dyn Transport,
    store:     &dyn CredStore,
    cache:     &dyn ModelCache, // per-provider model list; read on generation, written only by list-models (model-discovery.md §5)
    clock:     &dyn Clock,
) -> u8                          // the numeric exit; the bz shim materializes process::ExitCode (§8)
```

**`run` is the byte adapter over a typed core.** The canonical model (§3) is the contract; the NDJSON/JSON on the wire is one *serialization* of it. So the pure pipeline is exposed twice: a **typed** entry point a library embedder calls directly, and the **byte** `run` the CLI is built from.

```rust
fn generate(
    request: CanonicalRequest,   // the typed input — a parsed canonical request
    config:  ResolvedConfig,     // the resolved provider row + model + knobs (config §7)
    host:    &Host,              // the four data-plane seams (Transport/CredStore/ModelCache/Clock)
) -> impl Iterator<Item = Event> // the typed output — a lazy event stream, terminated by one End
```

`generate` IS the generation pipeline (model-cache resolve → encode → auth → send → frame → decode), yielding canonical `Event`s lazily and surfacing every failure **in-band** as an `Event::Error` (so the call is total — never a `Result` the caller threads). `run` wraps it: parse stdin bytes → `CanonicalRequest`, fold config → `ResolvedConfig`, then `pump` the events through the mode's sink (the byte adapter, §5.1) and return the exit. `--raw` is the one path NOT typed — it never decodes, so it stays a byte passthrough (`serve_raw`) outside `generate`. An embedder gets the typed events without the byte round-trip; the CLI gets exactly the same behavior, serialized (§9.8).

`stderr` is a third injected writer, not just `stdout`: the §5.9 errors that must reach the user but have no stdout home — the pre-sink fatals (flag parse 64, input-open 66, malformed config 78) and, under `--text`/`--thinking`, the in-band `Event::Error` the text projection suppresses from stdout — go there, so they stay testable (captured into a `Vec<u8>`) instead of the process's real stderr. `run` returns the numeric `u8` (the single-source-of-truth exit, §8); `main()` materializes the `process::ExitCode`.

`main()` is the ~12-line shim that restores SIGPIPE, snapshots real argv/env into `Args`, wires the real impls (`HttpTransport`, the XDG `CredStore`, the XDG-cache `ModelCache`, `SystemClock`), calls `run`, and maps its `u8` to `process::ExitCode`. **`main` is the only uncovered surface in the codebase**; everything testable lives behind `run`. The pipeline is `Iterator<Item = io::Result<Bytes>>` end to end — **blocking, never async**, no tokio, no `impl Stream`, no lifetime-parameterized stream types. A blocking, rustls-backed HTTP client streams chunk-by-chunk via `into_reader()`, so the pipeline is genuinely incremental without async color.

---

## 2. Non-Goals

- **Not an agent.** No multi-turn loop, no tool-execution loop, no retry/backoff. brazen *exposes* `retryable` but never acts on it; the caller orchestrates. This includes a **stale-cache 404** on the generation path: it fails with a hint to re-run `bz --list-models`, never an auto-refetch-and-retry (model-discovery.md §5.3).
- **Not stateful** beyond the sanctioned exceptions: XDG config, credential/token storage, and a **regenerable per-provider model-list cache** (`$XDG_CACHE_HOME`, written *only* by `bz --list-models`, read-only on the generation path — model-discovery.md §5). No history, no session files; the model cache is the lone derived-data store, and deleting it only forces the next `--list-models` to rebuild it.
- **No in-process fan-out.** One request per process (blocking transport). A caller that wants N concurrent requests spawns N `bz`.
- **No input-dialect auto-detection.** Input is canonical-by-default. No structural sniffing, no `--in-format`. `--raw` on input means "these bytes are already provider-native." A **positional prompt** (`bz "…"`, §5.5) is an *explicit* alternate input channel (argv, not stdin) selected by its presence — never by sniffing stdin. When present it **wins and stdin is not read at all** (the POSIX filter idiom: read input only when needed; an unread pipe is the writer's concern via `SIGPIPE`), so there is no two-inputs error and no tty probe.
- **No secrets-backend abstraction** (keychain/vault). Secrets are a 0600 JSON file; to use a vault, point an env var / config at an externally-injected value.
- **No verbosity/`--debug` flag.** Diagnostics ride the in-band error's `provider_detail`.
- **No lossless coverage of provider-unique features** in the canonical model. Logprobs, citations, cache breakpoints, safety settings ride `extra` in / `provider_detail`+`Raw` out, or require `--raw` (losing normalization). `--raw` is the one place "single representation" is knowingly bent.

---

## 3. The Canonical Model (single source of truth)

ONE request type and ONE event vocabulary are authoritative. OpenAI-chat, OpenAI-responses, Anthropic-messages, Google-genai, and Ollama are all **lossy projections** onto them and back. **The core never matches on a vendor name.** Three rules govern every decision and dissolve the hard cases without per-provider branches:

1. **A field is stored only if it cannot be computed.** `retryable`, "is this a non-stream response", "is the stream over" are queries, not fields.
2. **The empty set is not a special case.** `Thinking` exists canonically even on providers that lack it; `tools: []` is the no-tools path; usage fields are `Option` (`None`, never a fabricated `0`).
3. **One end token.** Every provider's terminator normalizes to exactly one `Event::End`. Refusal is a `Finish`, not an `Error`. Error is its own event.

### 3.1 The canonical Request

```rust
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CanonicalRequest {
    pub model: String,                  // canonical alias; the Provider row resolves it (computed, not a 2nd table)
    pub system: Option<Vec<Content>>,   // None = no system; Some(vec![]) is the same path, not special
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,               // empty = no tools; never Option
    pub tool_choice: ToolChoice,        // defaults to Auto
    pub parallel_tool_calls: Option<bool>, // lifted known knob; None = provider default. OpenAI top-level; Anthropic nests it in tool_choice
    pub max_tokens: Option<u32>,        // None; a provider row's body_defaults.max_tokens fills it at lowest precedence (§4.2), omitted when None and not required
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop: Vec<String>,              // empty = no stop sequences
    pub stream: Option<bool>,           // gen field, a tri-state HONORED end to end (§3.2/§4.4): fill_absent folds request > flag/env/file > row body_defaults > brazen's stream-native global default true. serve reads the resolved bool and carries the streaming intent to drive — never silently reverts it. stream:true wire-streams (framed decode); stream:false sends a single-JSON body folded whole by decode_full (config §4.2). The field is typed (not left to `extra`) to intercept the key so the resolved tri-state, not a passthrough false, decides the path. NOT how we detect stream-over (that's Event::End)
    #[serde(flatten)]
    pub extra: Map<String, Value>,      // adaptive thinking, reasoning_effort, safetySettings, … (the long-tail valve only)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Message { pub role: Role, pub content: Vec<Content> }  // ALWAYS a vec; a bare string decodes to vec![Text(..)]

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role { System, User, Assistant, Tool }

// CR-4: NO serde(tag="type"). The request parser needs a custom string-or-object decode for
// Text — a bare wire string ("hi") becomes Content::Text("hi"), an object decodes by its "type"
// discriminant — so Content uses that custom representation (not plain internal tagging, which
// cannot express a bare string and cannot serialize the Text(String) newtype). Content::Text(String)
// stays expressible both ways. This keeps the documented bytes (a bare string, or {"type":"text",…})
// and the type definition in agreement.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Content {
    Text(String),
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: Vec<Content>, is_error: bool },
    Thinking { text: String, signature: Option<String> },  // signature is LOAD-BEARING
    RedactedThinking { data: String },  // opaque, round-tripped verbatim (CR); the API 400s if altered/reordered/dropped
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Tool { pub name: String, pub description: Option<String>, pub input_schema: Value }

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    #[default] Auto,            // OpenAI "auto", Anthropic {"type":"auto"}
    Any,                       // OpenAI "required", Anthropic {"type":"any"}
    Tool { name: String },     // must call this one
    None,                      // tools visible but forbidden
}
```

**Reframes that dissolve branches:**

- **`content` is always `Vec<Content>`.** OpenAI's `"content":"hi"` and Anthropic's `"content":[{"type":"text",…}]` look like two shapes; they are one — a string is `vec![Content::Text(s)]`. The parser dissolves the distinction at decode time; nothing downstream branches on string-vs-list. `ToolResult.content` is likewise `Vec<Content>` (Anthropic allows text+image results; OpenAI sends a string) — same reframe.
- **`Role::Tool` exists even though Anthropic has no tool role.** Anthropic carries tool results inside a `user` message; OpenAI/Mistral use `role:"tool"`. Canonically there is ONE truth: `Role::Tool`. Each adapter owns its own projection — the core never branches on "which provider uses which tool convention."
- **`req.system` and `Role::System` are two *different* facts, not two homes for one.** `req.system` is the **leading, config-/flag-/file-sourced** system prompt (the ergonomic "data transported by bz", §5.5); `Role::System` is a system message at a **specific position** in a transcript a caller re-feeds verbatim. Each adapter projects both deterministically (Anthropic hoists either to its top-level `system`; OpenAI emits both in array order — see the mapping specs), so there is no dedup branch and no drift: the position *is* the distinguishing data. The empty case (`req.system: None`, no `Role::System` message) is the no-system path, not a special case. An auth mode may **mandate** that `req.system` lead with a preamble (a Claude-Code-scoped OAuth token rejects a request whose system does not begin with the Claude-Code line); that is auth-row data prepended to `req.system` in resolution — a body fact, so it cannot ride header-only `Auth::apply` (auth §4.1) — leaving `req.system` still the one leading-system home, now with a mandated lead.
- **`Thinking.signature` is `Option<String>` and must round-trip verbatim.** Anthropic thinking blocks carry an opaque `signature`; the API rejects modified/absent signatures on multi-turn replay. brazen is stateless, but the **caller round-trips** thinking blocks across turns through brazen, so the canonical model must carry the signature unmodified or it destroys the caller's ability to continue. `None` covers providers without the concept (the empty-set rule). Adapters never fabricate a signature — copy through or leave `None`.
- **`RedactedThinking { data }` is opaque and round-trips verbatim**, exactly like a signature. Anthropic emits `redacted_thinking` blocks whose `data` is an encrypted payload; the API 400s if `thinking`/`redacted_thinking` blocks are altered, reordered, or dropped on multi-turn replay, so the caller must round-trip them through brazen untouched. It is its own variant (not a lossy hack folded into `Thinking`) so the bytes are carried verbatim. Adapters without the concept simply never produce it (the empty-set rule).
- **`req.system` (`Option<Vec<Content>>`) and `ToolResult.content` (`Vec<Content>`) stay permissive** — the canonical model is the single source of truth and holds any `Content`. An adapter targeting a **text-only wire slot** (e.g. a provider whose system field or tool-result field accepts only text) that receives non-`Text` content **rejects at `encode`** with `ErrorKind::ParseInput` (exit 64) — a documented runtime degradation, not a type change. The permissive type stays one truth; the narrowing is the adapter's, surfaced as an error rather than a silent drop.
- **`ToolChoice` is a typed enum, not an `extra` knob** — all providers express the same four intents under different spellings ("lift known knobs explicitly"). The same rule lifts **`parallel_tool_calls: Option<bool>`**: OpenAI spells it as a top-level field, Anthropic as `tool_choice.disable_parallel_tool_use` — one canonical knob, each adapter owning its projection. It is *not* an `extra` key precisely because Anthropic nests it, which the top-level `extra` valve cannot reach.
- **Unknown top-level request keys are *forwarded*, not rejected.** `#[serde(flatten)] extra` is the long-tail valve (`reasoning_effort`, `safetySettings`, …): a key brazen doesn't model lands in `extra` and is passed to the provider verbatim. The cost, owned: a **misspelled** canonical field (`temperatue`) silently becomes a passthrough knob and surfaces as an upstream 4xx, not a local exit 64 — brazen does not validate the long tail.

### 3.2 The canonical streaming Response (the Event taxonomy)

**Output is a STREAM, always — even when the wire is not.** brazen's canonical output is a blocking incremental `Iterator` of `Event`s whatever the wire shape: a `stream:true` 2xx is a framed SSE/NDJSON stream, a `stream:false` 2xx is a single aggregate JSON body. The `stream` field is a tri-state HONORED end to end (§3.1, §4.4, config §4.2): `serve` reads the resolved bool — request > flag/env/file > row `body_defaults` > brazen's stream-native global default `true` — and CARRIES the streaming intent to `drive`, which routes the 2xx body through the matching fold. It is never silently reverted: a flag is either honored or errored, never ignored-and-overridden. The whole-body fold is the single-source mechanism here: a non-stream response IS the aggregate the stream emits, so each protocol's `decode_full` reconstructs the synthetic event sequence the stream would have produced and REPLAYS it through the SAME `decode`-internal helpers (explode→replay — no second parser). That fold — non-stream-response-IS-the-stream, the same `Event` vocabulary, the response stored once — is shared by every provider for BOTH the **non-2xx error body** (§3.4, §8: framed as one whole-body `Frame` carrying the status, decoded once via `decode`) AND the **non-stream 2xx body** (drained whole, decoded once via `decode_full`). Exact non-stream wire bytes — bypassing encode entirely — remain `--raw`'s territory (§5.4).

```rust
// CR-4: Event KEEPS serde(tag="type"). All its variants are struct/unit, and Usage/Error are
// newtype-of-STRUCT, which internal tagging handles. Event::Raw(Vec<u8>) is NEVER serde-serialized
// (raw mode writes bytes verbatim via RawSink, §5.4) — it is marked serde(skip) so it imposes no
// serde constraint on the tagged enum. Every open enum below is #[non_exhaustive] and carries an
// `Other` catch-all so the v=1 forward-compat contract (below) holds on BOTH surfaces — a new Rust
// variant never breaks a match, a new wire value never breaks a decode.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    MessageStart { v: u8, id: Option<String>, model: Option<String>, role: Role },  // v = event-schema version (currently 1)
    ContentStart { index: u32, kind: ContentKind },
    ContentDelta { index: u32, delta: Delta },
    ContentStop  { index: u32 },
    Usage(Usage),
    Finish { reason: FinishReason },
    Error(CanonicalError),
    #[serde(skip)]
    Raw(Vec<u8>),   // only under --raw; written verbatim by RawSink, never serde-serialized
    End,            // THE provider-agnostic terminator
    #[serde(other)]
    Other,          // an unknown event `type` decodes here (internal tagging's skip path), never an error
}

// CR-4: ContentKind uses EXTERNAL tagging, rendering "kind":{"text":{}} / {"tool_use":{…}} exactly
// as the §5.2 sample shows (internal tagging could not). Serialize/Deserialize are HAND-ROLLED (not
// derived): external tagging's derive has no #[serde(other)], so the forward-compat `Other(Value)`
// catch-all — which carries an unknown kind's whole {tag: body} object verbatim for passthrough — is
// dispatched by hand. The known variants render byte-identically to the former derive.
#[derive(Clone, Debug)]   // hand-rolled Serialize + Deserialize
#[non_exhaustive]
pub enum ContentKind {
    Text {},
    ToolUse { id: String, name: String },
    Thinking {},
    RedactedThinking {},
    Other(serde_json::Value),   // unknown kind, verbatim {tag: body} — never an error (the v=1 contract)
}

// CR-4: Delta uses EXTERNAL tagging so a newtype variant serializes as "delta":{"text_delta":"Hel"}.
// Like ContentKind it hand-rolls Serialize/Deserialize to add the `Other(Value)` forward-compat catch-all.
#[derive(Clone, Debug)]   // hand-rolled Serialize + Deserialize
#[non_exhaustive]
pub enum Delta {
    TextDelta(String),
    JsonDelta(String),       // tool-call argument fragments (string, NOT a parsed Value)
    ThinkingDelta(String),
    Other(serde_json::Value),   // unknown delta, verbatim {tag: body} — never an error
}

// Token counts; names are token-explicit (Anthropic input_tokens/output_tokens/cache_*_input_tokens,
// OpenAI prompt_tokens/completion_tokens) — frozen with the rest of the v=1 vocabulary.
// #[non_exhaustive]: a future counter is additive (out-of-crate: Usage::default() + field set).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
}

// No serde derive: FinishReason HAND-ROLLS Serialize/Deserialize into a FLAT map
// keyed on `reason` (not a `#[serde(tag)]` derive — a derived adjacently/internally
// tagged enum cannot emit the bare unit-as-string + sibling fields the wire needs).
// `Event::Finish` carries it `#[serde(flatten)]`, so the on-wire shape is
//   {"type":"finish","reason":"stop"}                         // every unit variant
//   {"type":"finish","reason":"refusal","category":"…","explanation":null}  // Refusal's two fields, flat siblings
//   {"type":"finish","reason":"<unknown>"}                    // Other(String): the literal string in `reason`
#[non_exhaustive]
pub enum FinishReason {
    Stop,                                                   // end_turn / "stop" / STOP / done
    Length,                                                 // max_tokens / "length" / MAX_TOKENS
    ToolUse,                                                // tool_use / "tool_calls"
    StopSequence,                                           // stop_sequence
    Refusal { category: String, explanation: Option<String> },  // arrives as HTTP 200, exit 0; category+explanation are flat sibling keys
    Pause,                                                  // Anthropic pause_turn (resumable agentic flow)
    Other(String),                                          // unknown reason — string passes through `reason` verbatim, never panics
}
// Serialize: serialize_map → "reason":"<variant>", plus "category"/"explanation" for Refusal.
// Deserialize: read {reason, category?, explanation?}, match `reason` → variant, unknown → Other(reason).
```

- **`MessageStart.v` is the event-schema version** — the one handshake a harness pins to. It is the first field of the first event on every non-`--raw`, non-error stream (currently `1`); a backward-incompatible change to the `Event` vocabulary bumps it, so a consumer can refuse a version it doesn't understand instead of mis-parsing. A stream that errors before any `MessageStart` carries no `v` — a consumer that gets `Error` first needs no version to act. `v` is stamped from a single `EVENT_SCHEMA_VERSION` const by the `Event::message_start` constructor — adapters build `MessageStart` through it and never retype the number, so it stays one source (the mapping specs map only `id`/`model`/`role`).
- **`ContentStart` and `ContentDelta` are deliberately separate** — block-open is not folded into the first delta. Anthropic streams `content_block_start` (carrying tool id/name *before* any argument bytes); OpenAI reveals `tool_calls[i].id`+`.function.name` on the first chunk. Keeping them separate lets the OpenAI adapter *synthesize* a `ContentStart{ToolUse{id,name}}` the first time an index appears, so **identity always precedes content for every block on every provider** — the consumer never needs a "did I see the id yet?" branch.
- **`Usage` fields are `Option`, never fabricated `0`.** A provider that never reports `cache_read_tokens` leaves it `None`; `0` would be a lie ("zero cache hits" vs "unknown"). Cumulative; emitted whenever a provider reveals it. The four fields are **token-explicit** (`input_tokens`/`output_tokens`/`cache_read_tokens`/`cache_write_tokens`) — they count tokens (mapping from Anthropic `input_tokens`/`output_tokens`/`cache_read_input_tokens`/`cache_creation_input_tokens` and OpenAI `prompt_tokens`/`completion_tokens`), so the names say so. The `{"type":"usage",…}` NDJSON line carries those exact keys (§5.2).
- **Refusal is a `Finish`, NEVER an `Error`.** A refusal arrives as HTTP 200 with `stop_reason:"refusal"`. Modeling it as an error would invent a second representation of "the request succeeded" and force a non-zero exit on a 200. `category` is `String` (open, growing set per the API) and `Other(String)` defends the top-level reason field — neither panics on an unknown value.
- **`ContentKind::RedactedThinking {}` mirrors the request-side `Content::RedactedThinking`.** Streamed redacted-thinking blocks open with this kind (carrying no streamed delta — the `data` rides the block's open/close). Adapters without the concept never emit it (the empty-set rule).
- **The `v=1` forward-compat contract — the vocabulary only GROWS within a `v`.** A consumer pinned to `v=1` **MUST ignore** an unknown event `type`, content `kind`, or `delta` variant — and unknown object fields — rather than erroring. So **adding** a new event/kind/delta (or a field on an existing one) is *additive*: it does not bump `v`. `v` bumps **only** for a removal, a rename, or a semantic change to an existing value. The types honor this on both surfaces: every open enum is `#[non_exhaustive]` (a new Rust variant never breaks a downstream `match`) and carries an `Other` catch-all so an unknown wire value **decodes** to `Other` instead of failing. The `Usage` struct is `#[non_exhaustive]` too — a new token counter is additive (a downstream reader keeps compiling; out-of-crate construction is `Usage::default()` + field assignment, since the struct literal is in-crate-only) — `Event::Other` (the `#[serde(other)]` skip path) drops the unknown payload; `ContentKind::Other`/`Delta::Other` carry it verbatim for passthrough; `FinishReason::Other` carries the unknown reason string; `ErrorKind::Other` carries the unknown error-kind tag. This dissolves what used to be a `FinishReason`-only tolerance into the general rule (`Other` is the general path; the named variants are its known cases), and serde already ignores unknown object fields by default. **The error event has no version gate, so it is tolerant by construction — NOT frozen.** `CanonicalError`/`ErrorKind` carry no `v` (an error-first stream emits no `MessageStart`, §3.3) — a consumer that gets `Error` first has no version to read. A shipped binary cannot be made tolerant retroactively, so the tolerance must ship *before* the 0.1.0 freeze: `ErrorKind` is `#[non_exhaustive]` **and** carries an `Other` catch-all (its hand-rolled serde routes any unrecognized snake_case `kind` to `Other`, verbatim), so the error schema grows *additively* under the very same `v=1` rule as the rest of the vocabulary — a future kind decodes to `Other`, never errors. Only a removal/rename/semantic change is forbidden (with no `v` to refuse it, it would silently break a pinned consumer, and is unfixable after ship).
- **Server-tool blocks are deferred (no canonical kind in v0.1).** Anthropic's `server_tool_use` and `web_search_tool_result` content blocks, and the `usage.server_tool_use.*` counters, have **no** canonical `ContentKind`/`Usage` field in v0.1; they ride `Raw` (under `--raw`) / `extra` / `provider_detail` rather than being normalized. Canonical kinds for them are **deferred until web-search support** lands — adding `ContentKind::WebSearchResult` later is the empty-set rule run forward and is purely additive under the `v=1` contract above (consumers already tolerate the unknown kind via `ContentKind::Other`; the new variant rides `#[non_exhaustive]`), **not a breaking change**.

### 3.3 Error — its own event, `retryable` computed

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CanonicalError {
    pub kind: ErrorKind,
    pub message: String,
    pub provider_detail: Option<Value>,   // parsed upstream error body, verbatim
    // NOTE: no `retryable` field — it is computed.
}

#[derive(Clone, Debug, PartialEq, Eq)]   // hand-rolled tolerant serde, NOT derived (see below)
#[non_exhaustive]
pub enum ErrorKind { Usage, ParseInput, Config, Auth, Provider { status: u16 }, Transport, Interrupted, Other(String) }
// Serialize: unit variants → snake_case string; Provider → {"provider":{"status":N}}; Other → its tag verbatim.
// Deserialize: known tag → variant; any unrecognized snake_case `kind` → Other(tag), never an error (§3.2 v=1 contract).

impl CanonicalError {
    /// retryable is a QUERY over kind, never a stored field that could drift.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::Transport)
            || matches!(self.kind, ErrorKind::Provider { status } if status == 429 || status >= 500)
    }
    pub fn exit_code(&self) -> u8 { /* sysexits mapping, see §8 */ }
}
```

Errors travel **in-band through the same Sink** as every other event — `MessageStart, ContentDelta, …, Error` is fully representable (partial response then mid-stream failure). There is no separate "error mode." `Error` is **never folded into `Finish`**: a response either finished (with some reason, possibly refusal) or it errored — two distinct truths, two distinct events.

### 3.4 The one terminator

`Event::End` is the single source of truth for "stream over" across **every mode except genuine `--raw` passthrough**. Every native terminator normalizes to exactly one `End`:

| Provider / protocol | Native terminator | → |
|---|---|---|
| Anthropic messages | `event: message_stop` | `End` |
| OpenAI chat/completions | `data: [DONE]` | `End` |
| OpenAI responses | `response.completed` | `End` |
| Google generative-ai | last chunk carrying `finishReason` | `End` |
| Ollama | `{"done": true}` (NDJSON) | `End` |

### 3.5 Derived vs stored (the single-source-of-truth ledger)

| Fact | Representation | Why |
|---|---|---|
| "stream is over" | **computed** — `DecodeState.terminated`, set when decode consumes the provider terminal marker (`[DONE]`/`message_stop`/…), NOT bare EOF | a clean stream and a premature drop both end in EOF; only the decoded terminal marker means "done" (CR-9) |
| "is this a non-stream response" | **carried, not guessed** — `serve` resolves the `stream` tri-state to a bool and threads the streaming intent into `drive` (§3.2, §4.4); `drive` folds a `!streamed` 2xx body whole via `decode_full`, a `streamed` one via the framed stream | carry-the-fact, not re-derive from the body shape; one decode vocabulary, response stored once |
| `retryable` | **computed** — `CanonicalError::retryable()` | two reps would drift |
| exit code | **computed** — `exit_code()` over `kind`/`io` | policy derives from `kind` |
| refusal-vs-success | **stored once** — `Finish{Refusal}` | inventing an Error duplicates "the 200 succeeded" |
| `Usage` zero vs unknown | **stored** as `Option` | `0` and `None` are different facts |
| model→provider routing | **computed** — alias resolved on the row | a second routing table would drift |
| `Thinking.signature` | **stored verbatim** | opaque, API-rejected if modified |
| tool-call `input` | **streamed** as `JsonDelta`; parsed to `Value` only when folding | the fragments are the source |
| block identity (id/name) | **stored once** in `ContentStart`; deltas carry only `index` | identity stated once |

Everything in a "computed" row is a method, not a field — so it cannot fall out of sync with its source.

### 3.6 How tool-calls and streaming reconcile without per-provider special cases

The core never asks "is this OpenAI or Anthropic?" Each `Protocol::decode` is a pure state machine translating its own dialect into the shared vocabulary:

- **Argument deltas are always `JsonDelta(String)`, never a parsed `Value`.** Both providers stream tool arguments as JSON *text fragments* valid only when concatenated; escaping differs across models, so parse with `JSON.parse()` after assembly — never string-match the serialized form. brazen carries fragments; assembly+parse is the consumer's job (or brazen's, only when folding to `Content::ToolUse{input: Value}`).
- **Indices unify positional and named blocks.** Anthropic gives the index; OpenAI gives a position in `tool_calls[]` plus a text slot. The adapter assigns one `index` space to both; downstream a `(ContentStart, ContentDelta*, ContentStop)` triple is identical regardless of origin.
- **`ContentStart`-before-deltas is an invariant both can satisfy** (Anthropic native, OpenAI synthesized).

**The executable single-source-of-truth check** (see §10): an OpenAI fixture and an Anthropic fixture for the *same logical response* decode to the *same* `Vec<Event>` (modulo provider-inherent identity and `Option` fields a provider genuinely doesn't supply). This is what proves the canonical model is one model, not two wearing a trenchcoat.

---

## 4. The Adapter Abstraction (Provider / Protocol / Auth + severability)

**Thesis:** a provider is a **row of data**; a protocol/auth is a **trait impl keyed by an enum id**; the pipeline **dispatches through a registry lookup, never a `match` on a vendor name.**

### 4.1 Three narrow, dyn-safe traits

All are object-safe — the pipeline holds `&dyn`. No generic methods, no `-> impl Trait`, no associated types in the call path.

```rust
/// Owns the wire dialect. Pure: no IO, no clock, no creds. Eight methods.
pub trait Protocol: Send + Sync {
    fn encode(&self, req: &CanonicalRequest, ctx: &ProviderCtx) -> Result<WireRequest, Error>;
    /// The request path appended to `base_url` (e.g. `/responses`) — DATA. `encode`
    /// builds its `wire.url` from this SAME path; `--raw`, which skips `encode`, calls
    /// it to fill `wire.url` (the path string has one home).
    fn path(&self, ctx: &ProviderCtx) -> String;
    /// The wire body's `Content-Type` — DATA, like `path`. `serve` stamps it for BOTH
    /// the encoded and the `--raw` path, so neither hardcodes the string.
    fn content_type(&self) -> &str;
    /// Consume ONE already-parsed frame -> zero or more canonical events.
    /// Statefulness (open-block indices, cumulative usage) is caller-owned `DecodeState`,
    /// so the impl is a pure fn of (frame, state) and shareable as &'static dyn.
    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, Error>;
    /// Decode a COMPLETE non-stream 2xx body -> the SAME events the stream yields
    /// (honors `stream:false`, config §4.2): not a second parser — it replays the
    /// aggregate through `decode`'s own helpers (§3.2).
    fn decode_full(&self, body: &[u8], state: &mut DecodeState) -> Result<Vec<Event>, Error>;
    /// Which transport framing this protocol uses. DATA, not behaviour.
    fn framing(&self) -> Framing;   // Sse | Ndjson | Identity
    /// The models-list endpoint appended to `base_url` for a GET — DATA, like `path`
    /// (model-discovery §3.1); the one GET `bz --list-models` makes.
    fn models_path(&self) -> &str;
    /// Decode the provider's models-list body into the canonical ORDER-PRESERVING
    /// list (model-discovery §3.1). PURE, fixture-tested like `decode`.
    fn decode_models(&self, body: &[u8]) -> Result<Vec<Model>, Error>;
}

/// The ONLY consumer of CredStore. The stateless boundary is drawn exactly here.
pub trait Auth: Send + Sync {
    fn apply(
        &self,
        wire: &mut WireRequest,
        ctx: &ProviderCtx,           // shared capabilities (base_url, model, beta_headers) — also handed to encode
        auth: &AuthCtx,              // auth-private: store key + inline secret + api_header + OAuth row data — NEVER handed to encode
        store: &dyn CredStore,
        clock: &dyn Clock,
        transport: &dyn Transport,   // for silent OAuth refresh — same seam, no new IO surface
    ) -> Result<(), Error>;
}

/// The ONLY impure seam. Real HttpTransport, test MockTransport.
pub trait Transport: Send + Sync {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, Error>;
}

pub struct TransportResponse {
    pub status: u16,                                          // peeked even under --raw, for exit-code correctness
    pub body: Box<dyn Iterator<Item = io::Result<Bytes>>>,   // blocking, incremental
}
```

`ProviderCtx` is the **read-only projection of the resolved row + flags** handed to encode/auth — the *entire* interface between "which provider" and "how to talk to it":

```rust
pub struct ProviderCtx<'a> {
    pub base_url: &'a str,
    pub model: &'a str,                          // already alias-resolved — encode never resolves aliases
    pub beta_headers: &'a [(&'a str, &'a str)],  // provider-level STATIC betas (e.g. anthropic-version)
}
```

The body-passthrough valve does **not** ride `ProviderCtx`. Config-level passthrough (the top-level `extra` map and a row's non-gen `body_defaults`, config §4.1) is folded into `req.extra` by `fill_absent` and reaches the wire through the one `req.extra` fold every encoder already runs — sharing the request's own valve rather than a second, encoder-unread `ctx.extra` (which earlier existed and was dead — config §9).

`ProviderCtx` carries **no `name`, no `ProtocolId`, no `AuthId`, no secret, and no `api_header`** — by the time encode/decode/auth run, the vendor identity has been spent on the registry lookup, and the auth header is auth's concern (it rides `AuthCtx`). The impls are vendor-blind; they see only capabilities.

`Auth::apply` needs four facts a vendor-blind `ProviderCtx` deliberately withholds: the **credential-store key**, the **auth header** to write, the **ambient discovery source** consulted on a store miss (auth §5.5), and, for OAuth, the **auth-row endpoints**. These ride a **second, auth-private projection**, `AuthCtx`, handed *only* to `apply` — never to `Protocol::encode`. The split is a **security boundary**: `ProviderCtx` is shared with `encode`, so a live credential placed there would be visible to the protocol layer; keeping the inline secret on `AuthCtx` makes "`Auth::apply` is the ONLY data-plane function permitted to touch credentials" (§6.5) a *type-level* fact rather than a convention. The `api_header` lives here for the same reason it is auth-only: `encode` has no business with it.

```rust
pub struct AuthCtx<'a> {
    pub store_key:  &'a str,                  // the provider name, used ONLY as a CredStore key — never matched on
    pub inline_key: Option<&'a Secret>,       // the §6.5 inline-key bypass; absent => store.get(store_key)
    pub api_header: Option<&'a HeaderSpec>,   // x-api-key | Authorization:Bearer | x-goog-api-key — DATA; Some iff a keyed row (None for AuthId::None)
    pub oauth:      Option<&'a OAuthConfig>,   // resolved auth-row data (§7); Some iff AuthId::OAuth2 (a resolve invariant)
    pub ambient:    Option<&'a AmbientSpec>,   // the row's ambient discovery source (auth §5.5); Some iff the row carries an `ambient` block — consulted on a store miss
}
```

`store_key` is a **key, not an identity** — the resolved provider name used solely to index `CredStore`, never a `match` on a vendor (the no-dispatch-on-name invariant of §4.4 holds). `api_header` is `Some` for every keyed row and `None` exactly for `AuthId::None`; `oauth` is `Some` exactly when the resolved row is `AuthId::OAuth2`; `ambient` is `Some` exactly when the row carries an `ambient` block (auth §5.5). Resolution pairs each keyed/OAuth field with its auth mode or errors (78), the same surfaced-ambiguity rule as model→provider routing (§4.3) — so `NoAuth` reads neither, `ApiKey`/`Bearer` read only `api_header`, and all four `Auth` impls stay stateless unit structs shareable as `&'static dyn`. Both contexts are projections of `ResolvedConfig` (`ProviderCtx::from(&cfg)` / `AuthCtx::from(&cfg)`).

### 4.2 Provider is DATA, not a trait

```rust
#[derive(Deserialize, Clone)]
pub struct Provider {
    pub name: String,            // table key only — never matched on in the pipeline
    pub base_url: String,
    pub protocol: ProtocolId,    // OpenAiChat | AnthropicMessages | (later) OpenAiResponses | GoogleGenAi | OllamaChat
    pub auth: AuthId,            // ApiKey | Bearer | OAuth2 | None
    #[serde(default)] pub api_header: Option<HeaderSpec>,  // { name:"x-api-key", scheme:Raw } | { name:"Authorization", scheme:Bearer } | None (auth = "none")
    #[serde(default)] pub beta_headers: Vec<(String, String)>,
    #[serde(default)] pub model_aliases: Map<String, String>,  // alias -> wire model id (computed lookup)
    #[serde(default)] pub model_prefixes: Vec<String>,         // owned model-id families for routing (§4.3): authored, consumed at route, not retained
    // the row's request-body defaults (config §4.1): gen params (max_tokens, stream, …) +
    // non-gen passthrough (store, …), the lowest-precedence operand in the fold. AUTHORED on the
    // row; CONSUMED into `ResolvedConfig` at resolve (gen scalars fold into the typed fields, the
    // rest into `extra`), so the resolved `Provider` need not retain it — config §4.1, §9.
    #[serde(default)] pub body_defaults: Map<String, Value>,
    // canonical fields the backend REJECTS (config §4.1.1): the inverse of body_defaults,
    // RETAINED on the resolved row (unlike body_defaults) because `strip_unsupported` reads it
    // at fill time to drop each from the request after `fill_absent`. Empty for standard rows.
    #[serde(default)] pub unsupported_body_keys: Vec<String>,
}
```

`ProtocolId`/`AuthId` are small closed enums but the pipeline never `match`es them — they are **registry keys**. The enum exists so config can name a protocol/auth in TOML with a typo-checked vocabulary; it is not a dispatch site. The built-in table is an embedded TOML string parsed through the **same** `resolve` path as user config — no bootstrap special case:

```toml
[[provider]]
name = "anthropic"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "api_key"
api_header = { name = "x-api-key", scheme = "raw" }
beta_headers = [["anthropic-version", "2023-06-01"]]
body_defaults = { max_tokens = 4096 }   # Anthropic requires max_tokens; the row's sane default (override via config/flag), config §4.1

[[provider]]
name = "openai"
base_url = "https://api.openai.com/v1"
protocol = "openai_chat"
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }

[[provider]]
name = "mistral"          # the ENTIRE Mistral diff. No code.
base_url = "https://api.mistral.ai/v1"
protocol = "openai_chat"  # Mistral speaks OpenAI chat/completions verbatim
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
```

### 4.3 Single source of truth: model→provider resolution

There is **no model→provider routing table** (a second home would drift). Resolution is a **query over the rows**, computed once during config resolution: the user names a provider explicitly (`--provider anthropic`) **or** brazen finds the single row that **owns** the model. A row owns the model when its `model_aliases` spell it (substitution shorthand) **or** one of its `model_prefixes` claims its family (e.g. anthropic owns `claude-`, openai owns `gpt-`/`o1`/`o3`/`o4`/`chatgpt-`) — either is enough, and both feed the one single-match query. Two owning rows is a `Config` error (78), never a silent pick — ambiguity is surfaced. Alias→wire-id substitution happens **in resolution**, so `ProviderCtx.model` is already the wire id and `encode` has no model logic.

The request's `model`, when set, is **request data** and wins for routing; only when the request omits it does `getConfigValue("model")` supply it (flag → env → config file, §6.1) — the request is not folded into config. **Alias substitution is `model_aliases.get(model).unwrap_or(model)`** — an unaliased string passes through *verbatim* (the user typed the real wire id), so alias tables are pure optional shorthand and may ship empty.

**Prefix ownership is what makes `--provider` droppable for an unmistakable id** (`bz -m claude-haiku-4-5-20251001 "q"` routes to anthropic with no flag): a versioned wire id no alias table could ever enumerate is routed by the *family* its row claims. Ownership covers *routing* only, distinct from alias *substitution* — a model that no row owns (matches no alias and no prefix) still requires an explicit `--provider`. Two rows that serve the same family stay opt-in: `openai-responses` ships **no** `model_prefixes` precisely because it serves the same OpenAI ids as `openai` over a second protocol, and claiming them would make every `gpt-…` ambiguous; `ollama`'s local model names have no stable family, so it too stays explicit.

**Imprecise models resolve against the model cache (no probe).** Ownership answers *routing* — which row to send to. It no longer also decides *is this a full wire id?*: every model string (full, partial, or absent) is resolved in `serve` against the provider's **cache** by the total `select_model` (model-discovery.md §4–§5) — an exact match, else a partial (first listed id containing it), else the **default** (absent model → first listed), else the string **verbatim** (cache miss/no match → attempt it literally). This is a **local file read, not a round-trip**; there is no probe and no `ResolvedConfig.probe`. **It does not relax routing**: a partial still cannot *pick* a provider (that would be a vendor-name table or an N-provider fan-out, both forbidden), so `bz -m opus "q"` with no `--provider` and no configured `provider` is still `NoProvider` (78) — the partial story "just works" once a provider is named or configured. Full mechanics: [model-discovery.md](model-discovery.md).

### 4.4 Dispatch with NO match-on-provider

```rust
pub struct Registry;   // zero-field handle; the two methods ARE the dispatch tables
impl Registry {
    pub fn builtin() -> Self { Registry }

    // A TOTAL match over the closed key-enum — exhaustiveness is the registration
    // guarantee. Returns the impl directly: no Option, no .expect(), no "unregistered
    // id" arm to panic on (a missing variant fails to COMPILE here, not at runtime).
    pub fn protocol(&self, id: ProtocolId) -> &'static dyn Protocol {
        match id {
            ProtocolId::OpenAiChat        => &OpenAiChat,
            ProtocolId::AnthropicMessages => &AnthropicMessages,
            ProtocolId::OpenAiResponses   => &OpenAiResponses,
            ProtocolId::GoogleGenAi       => &GoogleGenAi,
            ProtocolId::OllamaChat        => &OllamaChat,
            // adding a protocol = ONE match arm + ONE enum arm + ONE module. Nothing else.
        }
    }
    pub fn auth(&self, id: AuthId) -> &'static dyn Auth {
        match id {
            AuthId::ApiKey | AuthId::Bearer => &StaticSecretAuth, // one impl, two intent-naming ids (auth.md §3)
            AuthId::OAuth2                  => &OAuth2Auth,        // silent refresh + bz --login (§7); endpoints ride AuthCtx.oauth
            AuthId::None                    => &NoAuth,            // keyless (local Ollama): no cred, no header
        }
    }
}
```

The data flow through `run` — **no vendor name appears**:

The spine is the **byte adapter** `run/mod.rs` over the **typed core** `run/generate.rs`
(§1). `run/mod.rs` owns the **pre-sink** phase (flags → config fold → input → build the
sink), then for a canonical request `pump`s `generate`'s events into the sink; `generate`
owns the **request** half (cache lookup → encode → auth → send) and yields the response as
a lazy `Iterator<Event>` from `run/events.rs` (status→exit-carrying errors, frame→decode).
`--raw` is the one path outside the typed core: `run/serve.rs`'s `serve_raw` streams bytes
verbatim (it never decodes). The walk-through below shows that shared request-half logic;
on the canonical path it lives in `generate`, on `--raw` in `serve_raw`.

```rust
// ---- run/mod.rs: pre-sink (fatal, stderr-only). output mode is body-independent, so it's resolved FIRST. ----
let merged = flags.config.or(env_partial).or(file).or(defaults());  // the fold; NOT a layer: the request
let output = merged.output.unwrap_or(OutMode::Text);                // --text | --json(Ndjson) | --raw
let raw    = output == OutMode::Raw;
let reader = open(&flags.input)?;                                   // --input FILE (66 on open-fail) else the injected stdin
let mut sink = sink_for(output, thinking, stdout, stderr);         // the Sink exists from here: every later failure is in-band (§8)
if raw { return serve_raw(reader, merged, &mut *sink, host); }      // --raw: the byte passthrough (run/serve.rs)
let request = match read_request(prompt, reader) { Ok(r) => r, Err(e) => return fail_inband(sink, e) };
let cfg = merged.into_resolved(req_model(&request))?;              // 78 → fail_inband; else hand the typed core the request
pump(generate(request, cfg, host), &mut *sink)                    // the byte adapter: serialize generate's events, fold the exit

// ---- run/generate.rs: generate — the typed core's request half. Every error is an in-band Event::Error. ----
let input = if raw { Input::Raw(read_to_vec(reader)?) }            // --raw: stdin bytes verbatim, no parse
            else    { Input::Canonical(read_request(prompt, reader)?) };  // positional prompt wins; else stdin
let req_model = match &input { Input::Canonical(r) if !r.model.is_empty() => Some(r.model.clone()), _ => None };
let mut cfg = merged.into_resolved(req_model.as_deref())?;         // routes the row, substitutes the alias (no probe — model-discovery §5)

// Model resolution against the CACHE (model-discovery §5.2): every model string — full, partial, or
// absent — is resolved against the provider's cached list. A hit (exact/partial/default) gives the wire
// id; a miss/no-match falls through to the seed VERBATIM. A local FILE READ, not a round-trip; the only
// error is empty-seed + empty-cache (Config 78). --raw skips it (encode bypassed, the model is never read).
if !raw {
    let models = cache.get(&cfg.provider.name).unwrap_or_default();   // miss → empty list
    let (wire, prov) = select_model(&models, &cfg.model)?;            // §4: Cached | Verbatim
    cfg.model = wire;
    cfg.model_from_cache = matches!(prov, Provenance::Cached);        // carried for the 404 hint (§5.3)
}

let proto = registry.protocol(cfg.provider.protocol);   // total match on the closed key-enum, never a vendor name
let auth  = registry.auth(cfg.provider.auth);           // infallible: returns the impl directly, no Option
let ctx   = ProviderCtx { base_url, model: &cfg.model, beta_headers };   // shared, secret-free (also given to encode)
let authc = AuthCtx  { store_key, inline_key, api_header, oauth, ambient };  // auth-private

// `streamed` is the wire's streaming intent, CARRIED to drive so it folds the 2xx body by the shape the
// request ASKED for, not by guessing it back (carry-the-fact, §3.5). --raw has no parsed body → stays true.
let mut streamed = true;
let mut wire = match input {
    Input::Raw(bytes)   => WireRequest::new(format!("{}{}", ctx.base_url, proto.path(&ctx)), bytes), // SAME target encode builds
    Input::Canonical(mut req) => {
        fill_absent(&mut req, &cfg);          // config fills ONLY fields the request omits
        lead_with_preamble(&mut req, &cfg);   // an auth mode may mandate a leading system preamble (auth §4.1)
        strip_unsupported(&mut req, &cfg);    // drop fields the routed backend can't accept (config §4.1.1)
        streamed = req.stream.unwrap_or(true);  // the resolved tri-state → concrete bool; brazen's stream-native default
        proto.encode(&req, &ctx)?
    }
};
wire.set_header("content-type", proto.content_type());  // the dialect's media type — ONE home (Protocol::content_type), stamped for BOTH paths; --raw needs it or a JSON-body provider can't parse the verbatim body (bl-da81)
for (k, v) in ctx.beta_headers { wire.set_header(k, v); }  // the row's STATIC betas (e.g. anthropic-version) — ONE home, stamped for BOTH paths; --raw needs them or Anthropic 400s on the missing version header (bl-3e2f)
wire.timeouts = cfg.timeouts();             // stamp resolved transport timeouts (both paths), BEFORE auth's own token POST inherits them
auth.apply(&mut wire, &ctx, &authc, store, clock, transport)?;  // the one cred seam
let resp = transport.send(wire)?;                              // the one IO seam
response_events(proto, resp, streamed, hint).chain(once(Event::End))  // generate's typed output; pump (run/mod.rs) serializes it

// ---- run/events.rs: response_events — the response as a LAZY Iterator<Event> (no sink; the exit rides the errors). ----
let mut state = DecodeState::default();      // carries `terminated: bool`, set when decode consumes the terminal marker
if !is_2xx(status) {
    whole_body(...)                  // non-2xx: drain the WHOLE body as ONE error Frame (frame.status: Some), decode → Event::Error carrying the status's exit (sse §9, §4.3)
} else if !streamed {
    decode_full(...)                 // non-stream 2xx: drain whole, hand to proto.decode_full — the explode→replay of the streamed sequence (no framing, no EOF check)
} else {
    StreamEvents::new(...)           // streamed 2xx: a PULL iterator — each next() pulls a chunk through the framer; clean EOF with no terminal marker yields premature-EOF (CR-9, 69)
}
// No status seed: a non-2xx ALWAYS decodes to an Event::Error carrying its exit, so `pump`'s last-error-wins fold
// (§8) over the events yields the right code. `--raw` is the exception (it decodes no error) — `serve_raw` seeds
// the exit from the status (§5.4). `generate` chains the single trailing `End`; `pump` writes it and returns the exit.
exit
```

The only enums the core touches are **registry keys**, dispatched by a total match over the *closed `ProtocolId`/`AuthId` key-enum* (compiler-enforced completeness — strictly more in the spirit of "no match-on-name" than a partial runtime map, since a missing impl can't compile, let alone panic), never a vendor name; the branches in the spine itself are on `Input` being raw-or-parsed and, in `drive`, on the response's *shape* (`raw`, 2xx, the carried `streamed`) — *modes*, never a vendor. Exactly one place knows specific providers: `Registry`, the severable seam itself.

**Output mode gates input.** The output projection (`--text`/`--json`/`--raw`) appears only in flags/config, **never in the request**, so it is body-independent and resolved *first* — it decides whether stdin is parsed as a canonical request or passed through verbatim under `--raw`. The request itself is never a config layer — it contributes only its own data (below).

**The pipe is clean data; config fills gaps.** `model`, `max_tokens`, `temperature`, `top_p`, and `stop` are *request* fields. A field the request **sets is used as-is** — the body is never a config-precedence layer an invoker must reason about. For a field the request **omits**, `fill_absent` supplies `getConfigValue(field)` = **flag → env → config file → app/row default** (`--config` only changes *which* file, §6.3; a direct flag still beats that file). So per field the effective order is **request > flag > env > config > default**, expressed as two mechanisms — the request, and config-fills-the-rest — never one fold the caller must learn. **`stream` follows the same fill, with brazen's stream-native global default `true` as its lowest operand** (`req.stream.or(cfg.stream).or(Some(true))`, config §4.2): the resolved bool is HONORED, never force-reverted — `serve` carries the streaming intent to `drive` so a `stream:false` body is folded whole by `decode_full` (§3.2). A provider that works better non-streamed pins `body_defaults = { stream = false }` (policy in the row, not core; `--raw` still bypasses encode for exact wire bytes). `encode` then reads every gen param off `req` and the resolved wire `model` off `ctx`; `req.system` is filled the same way; structural payload (`messages`, `tools`) is the request's alone. `req.extra` is the request's own long-tail valve, but `fill_absent` seeds config passthrough (top-level `extra` + a row's non-gen `body_defaults`) beneath it at lowest precedence — a request `extra` key still wins (config §4.1).

### 4.5 Auth-mode-dependent headers live on the Auth impl, not the row

The Anthropic `anthropic-beta: oauth-2025-04-20` header differs **by auth mode on the same provider** (api-key vs OAuth on `api.anthropic.com`). A per-provider-only field cannot express "this header only under OAuth" without a core branch. So:

- **Provider row** carries auth-mode-*independent* headers (`anthropic-version`) — always sent.
- **`OAuth2Auth::apply`** adds `Authorization: Bearer …` **and** the auth row's `beta_headers` — **DATA, not a literal**: it iterates `AuthCtx.oauth.beta_headers` (`e.g. anthropic-beta: oauth-2025-04-20`, §4.1), it does not hardcode the string — and performs the silent refresh. OAuth knowledge is fully contained in one `Auth` impl; the beta header *value* lives once, on the row.

### 4.6 Severability proof (the grading rubric)

- **Add Mistral** (new provider, existing protocol+auth): **one `[[provider]]` row, zero Rust.** Delete the row → gone.
- **Add OpenAI "responses"** (new dialect): `mod openai_responses` (`impl Protocol`, pure, fixture-tested) + one `ProtocolId` arm + one `Registry::protocol` match arm. **Nothing in `run`, `resolve`, `parse`, the Sink, the canonical model, or the other Protocol impls changes** — `response.completed` normalizes to the same `Event::End`. Delete module+arm → gone; the registry match then fails to compile until the dead `ProtocolId` arm is removed too (the exhaustiveness guarantee, run in reverse), and rows that referenced it fail at resolve with a `Config` error.
- **Add Google's `x-goog-api-key`**: already expressible as `HeaderSpec { name:"x-goog-api-key", scheme:Raw }` on the row; `StaticSecretAuth` reads `auth.api_header` by data — no branch, no new impl.
- **Add a keyless provider** (local Ollama): `auth = "none"` and no `api_header` on the row — `NoAuth` reads no credential and writes no header. No `--api-key`, no `bz --login`; a stray `--api-key` is ignored. The keyless dual of the keyed rows' "missing key → 77".

The invariant that holds it all: **the core's only knowledge of a provider is `cfg.provider.protocol` / `cfg.provider.auth` as map keys.** `name` never reaches a dispatch site.

---

## 5. I/O & Streaming & POSIX Contract

brazen is a **strict unix filter**: deterministic, line-oriented, unbuffered-per-event, signal-correct. The request arrives two ways — a **positional prompt** (`bz "what is 2+2"`) or a **stdin** canonical request (§5.5) — and output is a **projection** chosen by flag: `--text` (default, human-readable), `--thinking`, `--json` (the full NDJSON event stream harnesses consume), or `--raw` (§5.1–5.4). So `bz "what is 2+2"` → `4`; `bz "what is 2+2" --json` → the event stream.

### 5.1 The single output seam

```rust
/// The one output surface. `write` is called once per Event, in order.
/// Implementors MUST flush before returning — no event is buffered across calls.
trait Sink { fn write(&mut self, ev: &Event) -> io::Result<()>; }
enum OutMode { Text, Ndjson, Raw }   // default = Text (human-ergonomic); --json -> Ndjson, --raw -> Raw
```

The byte adapter `pump` (the production driver loop, §10) drives `generate`'s typed event stream through the sink: mode-agnostic, computing the exit as it writes each event (last-error-wins); `Event::Error` does **not** stop the loop (errors are in-band; partial-response-then-error is representable). A sink write failure maps through the single-source `ExitClass::from_io` (`BrokenPipe` → 141, §5.8). The `--raw` and pre-stream-fatal paths push to the sink via `write_event`/`fail_inband` instead of `pump`, but share that SAME `from_io` mapping — the BrokenPipe→141 fact has one home.

### 5.2 stdout framing — NDJSON (`--json`)

**One canonical `Event` per line, `\n`-terminated, flushed immediately after each line.** NDJSON is `serde`'s direct serialization of the `Event` enum — no second schema, no hand-written framing grammar; a new variant needs zero framing change. The frame boundary is `\n`; serde escapes embedded newlines inside strings, so a line break is always a frame boundary. UTF-8 only. Each line is serialized to a `Vec<u8>`, `\n` appended, written with one `write_all`, then `flush()` — never a partial line on the wire.

```rust
impl Sink for NdjsonSink {
    fn write(&mut self, ev: &Event) -> io::Result<()> {
        let mut buf = serde_json::to_vec(ev).expect("Event is infallibly serializable");
        buf.push(b'\n');
        self.w.write_all(&buf)?;
        self.w.flush()
    }
}
```

(The `expect` is on our own owned `Event`, not external input — the one permitted internal infallibility, consistent with the `unwrap_used` deny on the data path.)

Sample wire shape (the **fixture bytes** the §10 tests assert against are byte-identical to this):

```
{"type":"message_start","v":1,"id":"msg_01…","model":"claude-3-5-sonnet","role":"assistant"}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}
{"type":"content_stop","index":0}
{"type":"usage","input_tokens":12,"output_tokens":2,"cache_read_tokens":null,"cache_write_tokens":null}
{"type":"finish","reason":"stop"}
{"type":"end"}
```

The `"kind":{"text":{}}` and `"delta":{"text_delta":"Hel"}` shapes are **externally tagged** — this is the resolution of **CR-4** flagged by both mapping specs: `ContentKind` and `Delta` drop internal tagging (`serde(tag=…)`) precisely so the type definitions (§3.2), this sample, and the committed fixture bytes all agree. `Event` keeps `"type"` internal tagging (its outer envelope above), and `Event::Raw` is `serde(skip)` so it never appears here.

### 5.3 Output projections — `--text` (default), `--thinking`, `--json`

**`--text` (default).** Human/REPL mode: emit only `ContentDelta::TextDelta` bytes, concatenated, no framing, no injected trailing newline. Thinking/tool/usage/start events drop. `Finish`/`End` produce no stdout bytes (they still set the exit code). **`Event::Error` is written to stderr** (its `message`, one line) so a mid-stream provider failure is never silent — text mode suppresses event lines from *stdout*, not from the user; the exit code still derives from it. Flush per delta. Terminator is **EOF on stdout** (an in-band `end` line would corrupt human output) — one of the two modes where `Event::End` is not the on-wire terminator.

**`--thinking`.** As `--text`, but `ContentDelta::ThinkingDelta` text is also emitted, *before* the answer, followed by a single `\n` separator at the first non-thinking content: `bz "2+2" --thinking` → `…reasoning…\n4`. This is the lone place text mode injects a separator; any finer structure lives in `--json`.

**Pretty text on a tty (interactive skin).** On an interactive terminal the default `--text` mode gains a strictly-additive pretty skin: the **answer on stdout stays byte-identical and unstyled** (the building-block contract above), while human chrome — tool-call lines, a finish/usage footer, styled errors — goes to **stderr**, and `--thinking` reasoning stays on stdout merely wrapped in dim SGR. The lib stays tty-blind: the **stdout**-isatty fact rides `Args.stdout_tty` (the sibling of the `Args.tty` stdin probe, §5.5), and a pure `Style::resolve(stdout_tty, env)` owns the activation predicate (`stdout_tty ∧ Text ∧ NO_COLOR unset ∧ TERM≠dumb`) and every glyph. `--json`/`--raw` are never prettified. See [interactive-output.md](interactive-output.md).

**`--json`.** The full NDJSON event stream of §5.2 — the contract harnesses build on (tool-call `JsonDelta` fragments, `Usage`, block ids, `MessageStart.v`). Everything the text projections drop is here, losslessly, and errors stay in-band on stdout as `Event::Error`.

### 5.4 `--raw` passthrough

The single, knowingly-bent place where normalization is skipped:

- **Decode is identity.** Transport bytes become `Event::Raw(Bytes)` chunks; `RawSink` writes them verbatim, flushing per chunk.
- **The provider's own terminator stands.** brazen does **not** append `{"type":"end"}`.
- **`--raw` is symmetric on input**: stdin bytes are already provider-native and go to transport verbatim (no `parse`, no `encode`). The encode/auth/transport middle is byte-identical to the normalized path — raw is "skip the two translators," not a parallel pipeline. The **body** is verbatim, but the **wire-level headers still ride**: skipping `encode` skips neither the URL, the auth headers, the content-type, nor the row's **static `beta_headers`**. The URL still targets `{base_url}{path}` (`Protocol::path`); `Auth::apply` still adds the auth headers; and `serve` stamps both `Protocol::content_type()` — the dialect's media type — and the row's `ctx.beta_headers` (e.g. Anthropic's mandatory `anthropic-version`), each ONE home read by both paths — so a verbatim JSON body is parsed by a JSON-body provider (without the content-type openai `chat/completions` 400s the content-type-less POST, bl-da81; without the version header every Anthropic raw request 400s, bl-3e2f). Each of these is the SAME single home the encoded path reads; raw inherits them, it does not send a bare bodyless wire. (Auth-mode-DEPENDENT betas — e.g. an OAuth row's `anthropic-beta` — ride `Auth::apply`, not this static set.) `--raw` is **symmetric-only** at 0.1.0 (both directions, no `--raw=in`/`--raw=out` split); that decision and the forward-compatible deferral of a directional split are settled in §5.10.2 / §13.14.
- **HTTP status is still peeked**: a raw 4xx/5xx sets the exit code per §8 even though the body streams raw and no `Event::Error` line is emitted. **A raw 4xx/5xx MUST NOT exit 0** — the one rule `--raw` does not bend.

### 5.5 Input: real pipe vs `--input FILE` (identical path)

The file-vs-pipe distinction **dies at construction** and never becomes a branch:

```rust
fn open_input(flags: &Flags) -> io::Result<Box<dyn Read>> {
    Ok(match &flags.input {
        Some(path) => Box::new(File::open(path)?),  // "simulated pipe"
        None        => Box::new(io::stdin().lock()),
    })
}
```

A file's EOF and a closed pipe's EOF are the same `Ok(0)`. **Parity is a test invariant** (§10). `--input -` is **not** special-cased (no second name for stdin). A missing/unreadable `--input FILE` is exit **66** (`EX_NOINPUT`), distinct from malformed *content* (64).

**Positional prompt — the operand tail, through-EOF.** The prompt is the **first, last, and only** positional argument: option parsing **stops at the first argument that is not an option (or an option-argument)**, and **everything from there through EOF is the prompt** — the operand arguments joined by a single space. So `bz what is 2+2` sends `"what is 2+2"` unquoted, and any `-`/`--`/word *after* the prompt begins is **inert text, never an option** (`bz what does --json do` sends the literal words). The corollary — the **brick wall** — is that **options must precede the prompt** (POSIX Utility Syntax Guideline 9, options-before-operands): `bz --json "q"` selects JSON, but `bz "q" --json` sends the prompt `"q --json"`. A leading-dash prompt uses `--` (`bz -- --weird`); if the first non-consumed argument begins with `-` and isn't `--` it is an option (an unknown one → 64), so a leading-flag typo is still caught — clear skies for the operand tail, a brick wall at the front. `read_request` builds `CanonicalRequest{ messages:[User Text(prompt)] }` from that tail and **does not read stdin at all** — the POSIX filter idiom: a program reads stdin only when it needs it, and a reader that stops early leaves the unread remainder as the *writer's* concern (`EPIPE`/`SIGPIPE` on its next write), like `head`. So a positional prompt simply **wins**: any piped stdin is silently not consumed (the positional is the explicit signal — no sniffing, no "silent pick", and **no two-inputs error**), and `bz "hi"` never blocks on or probes an interactive tty. `system`, `model`, and the gen params come from config/flags (merged in §4.4), so `bz what is 2+2` against a configured provider/model is a complete invocation. A harness composing tools/thinking/multi-turn pipes a full canonical request on stdin instead (no positional). Both funnel into the same `CanonicalRequest` — the positional form is a *constructor*, not a second request type.

There is therefore **no prompt-vs-stdin drain and no XOR error** — a present positional wins and `read_request` never touches the reader (above). The only tty concern is the **no-positional** path: bare `bz` (no prompt, no `--input`) calls `parse(stdin)`, and **an interactive tty never reaches EOF**, so that read would hang. Resolution: the `bz` shim probes `isatty(0)` (an impurity kept out of the pure lib, sibling of `restore_sigpipe`, §5.8) and, when stdin is a tty, hands `read_request` an **empty** reader instead of the real stdin; `parse` then sees `Ok(0)` and `run` prints the friendly bare-invocation hint below (exit 64) instead of blocking. A **genuine pipe** (non-tty, e.g. `echo '{…}' | bz`) flows through and is parsed as a canonical request, unchanged. The probe is `#[cfg(unix)]`; non-Unix treats stdin as always present (no tty hang in scope). The lib stays tty-blind on the *read* path — the seam is which reader the shim injects — but **the tty fact is also carried on `Args.tty`** (the same impurity-injection bundle as argv/env, §6.5), the one parameter the pure `run` reads to decide the friendly-bare hint below; the shim's `isatty(0)` probe feeds both. `Args.tty` is `false` for the non-Unix and the pipe cases, so neither path changes.

**Discovery short-circuits and the friendly bare invocation.** Two flags answer **before any config read or network** (a probe must respond even with a broken config or no provider), so they short-circuit in `run` as siblings of `--dump-config`, each writing to **stdout** and exiting **0**: `--help`/`-h` prints a one-screen usage (synopsis; the prompt-XOR-stdin input model; the `--login`/`--list-models` control flags; the flag list; the §8 exit-code table) and `--version`/`-V` prints the package version (`CARGO_PKG_VERSION`, the single source). `--help` wins over `--version`. A closed stdout (`bz --help | head`) maps through `from_io` to SIGPIPE/141, never a silent 0 — the same write-and-flush as `--dump-config`. Separately, a **bare interactive invocation** — stdin is a tty (`Args.tty`), **and** there is no positional prompt and no `--input FILE`, so there is no request source at all — would otherwise hit the empty-stdin parse error; instead `run` writes the same usage text to **stderr** and exits **64** (the usage class). This is the *only* place `Args.tty` changes behavior; the pipe/script path (`Args.tty == false`) still parses empty/malformed stdin as the 64 content error, unchanged. An unknown flag's 64 error also points at `bz --help` so a typo is recoverable. (`--help`/`--version` are flags of the one `parse_args`, short-circuiting identically in `run`/`list_models`/`login`; the shim keys its per-mode seam wiring on the `--login`/`--list-models` control flag, no longer on `argv[0]` — §5.10.1. So `bz --login --help` self-describes via the same shared doc.)

### 5.6 Termination / the end token

- **NDJSON: the end token is the literal line `{"type":"end"}`**, emitted exactly once, last, after any `Finish`/`Usage`/`Error`. **`Finish` ≠ end**: `Finish` is *why* generation stopped; `End` means *the byte stream is over*. A refusal is `Finish{refusal}` + `End`, exit 0. A consumer reads lines until `type == "end"`, then expects EOF.
- **Premature upstream EOF** → an in-band `Event::Error{kind:Transport}`, then `Event::End`, then exit 69. But a **clean** stream also ends in EOF, so this injection is **conditional on `DecodeState.terminated`** (**CR-9**): `decode` sets `terminated = true` when it consumes the provider terminal marker (`[DONE]` / `message_stop` / `response.completed` / `{"done":true}` / a `finishReason`-bearing final chunk). After the body iterator drains, `run` injects the premature-EOF `Error{Transport}` + exit 69 **only if not `terminated`** — a decoded terminal marker **suppresses** the injection. `decode` still **never** emits `End`; `run` owns the single `End` unconditionally. **An NDJSON stream always ends with `end`, even on failure** — one invariant dissolves the "did it finish?" edge case.
- `--text`/`--thinking`/`--raw` terminate by **EOF on stdout**.

### 5.7 Flushing & backpressure

Flush after every event — no `BufWriter` accumulation. Backpressure is the kernel's pipe buffer honored by blocking writes: `write_all` blocks when the downstream pipe is full, and because the pipeline is a blocking `Iterator`, we don't pull the next transport chunk until the current event is flushed. No internal queue, no dropped events, no unbounded memory. This is *why* the blocking spine is correct: backpressure is free and end-to-end. We never set stdout nonblocking.

### 5.8 Signals — one mechanism per OS (mutually exclusive)

- **Unix: restore `SIGPIPE` to `SIG_DFL` at startup.** Rust defaults to `SIG_IGN`; we undo it in the `main` shim (one `unsafe libc::signal` call). A write to a closed stdout then kills the process by signal — like `cat | head` — exit **141** (128+13). We never reach a `BrokenPipe` write-error branch.
- **Windows: no SIGPIPE.** `write_all`/`flush` returns `BrokenPipe`, which `ExitClass::from_io` maps to the same exit **141** — the single mapping the production byte adapters share (`pump` on the canonical stream, `write_event`/`fail_inband` on the `--raw`/pre-stream-fatal paths, §5.1). The only place the path differs.
- **SIGINT → 130, SIGTERM → 143** by default disposition — we install no handlers (nothing stateful to unwind; creds are written synchronously inside `Auth::apply` before any streaming). Already-flushed NDJSON lines stay valid. Determinism via *absence* of mechanism.

```rust
#[cfg(unix)]    unsafe fn restore_sigpipe() { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
#[cfg(not(unix))] fn restore_sigpipe() {}
```

### 5.9 stderr

Silent on the happy path. stderr carries a fatal condition that prevents the stream from starting *and* cannot be in-band — flag/usage parse failure (exit 64) and input-open failure (exit 66), both **before any Sink exists** — **plus** the one in-band error with no stdout home: under `--text`/`--thinking`, `Event::Error` (§5.3), since the text projection suppresses event lines from stdout. In NDJSON mode errors are in-band `Event::Error` on stdout; under `--raw` a 4xx/5xx shows only in the exit code (§5.4). The rule holds: a given failure appears in **exactly one** place — stderr only when stdout cannot carry it.

### 5.10 The committed CLI surface (frozen at 0.1.0)

The CLI **is** the product, and a shipped surface is the hardest door to change — every script that calls `bz` pins it. This section is the **one-way-door contract**: the complete argv / stdin / exit surface, declared frozen at 0.1.0. Two shapes are settled here; their owner-decided provenance + rationale live in §13.13 (control verbs → flags) and §13.14 (`--raw` symmetry). The rest enumerates what is committed and states the **one rule** that keeps the bare-prompt namespace from ever shrinking again.

#### 5.10.1 Control operations are flags, not verbs — and the bare-prompt namespace is total and frozen

`bz "what is 2+2"` is the charismatic core of the product: a **bare leading word is a prompt**. The danger is that control operations used to share the same argv slot. Pre-0.1.0 the shim dispatched on `argv[0]` — `login` and `list-models` were verbs, everything else a positional prompt — so two consequences followed, both bad:

1. `bz login` could **never** be the prompt `"login"`; `bz list-models` never the prompt `"list-models"`.
2. Every *future* top-level verb would **permanently shrink** the set of bare prompts that work: shipping `bz models` later would silently break everyone who today runs `bz "models"`. A one-way door that keeps taking.

**Resolution: control operations are flags in the existing control-short-circuit family, never verbs.** The codebase already expresses control operations this way — `--help`, `--version`, and `--dump-config` are flags that *replace* the data-plane run with a control action and exit, each with its own output shape and no request body. `login` and `list-models` were the inconsistent outliers. Folding them in (`--login`, `--list-models`) **dissolves the special-case `argv[0]` dispatch into the general flag path** (AGENTS.md: "dissolve special cases"; "a new verb is a smell — prefer an existing explicit signal"). `--dump-config`'s own existence refutes model-discovery.md §2's "why a verb, not a flag" argument: a flag *can* be a distinct mode with its own output and no body — it short-circuits in the flag layer rather than no-op-ing the request pipeline.

**The frozen rule (the namespace invariant):**

> The leading positional argument — the first argv element not starting with `-`, or **any** argument after a `--` opts-terminator — is **always** the prompt. It is never a verb and never a mode selector. Control operations are **always** flags (the `--login` / `--list-models` / `--dump-config` / `--help` / `--version` family) and a flag cannot collide with a prompt: flags start with `-`, and a literal `-`-leading prompt is reachable after `--`. Therefore the set of strings that work as a bare prompt is **every string**, and it **never shrinks**. A new control operation is a new flag — never a reserved word. `bz "models"`, `bz "login"`, `bz "list"` are valid prompts today and forever.

This is the one-way door we are deliberately **not** walking through.

**Committed control shapes** (provider via the existing `--provider` flag — one provider-resolution path for all three modes, §13.13):

```
bz --login --provider <id> [--browser]   # obtain+store an OAuth/SSO cred (the one interactive surface)
bz --list-models [--provider <id>]        # one GET: list the resolved provider's models
bz --dump-config                          # print the merged config as TOML, exit 0
bz --help | -h   /   bz --version | -V    # self-describe, exit 0
```

- **`--login`** requires a resolvable provider (it authenticates a *specific* provider, and there is no model to route from): the provider resolves through the SAME fold as a normal run (`--provider`, else a configured provider), and none-resolved is the usual 78/64. `--browser` selects the loopback browser flow (else the headless device flow); `--browser` is meaningful only with `--login`.
- **`--list-models`** resolves its provider exactly as the data plane does (`--provider`, else the row owning a configured `model`; neither → 78). Its output shape is the resolved `OutMode` (`--json` ⇒ the `{"models":[…]}` object, else ids one per line).
- **Mutual exclusion / precedence.** The two *probes* (`--help`, `--version`) answer before any config or network and win first (`--help` over `--version`) — a probe must respond even with a broken config. The control operations (`--dump-config`, `--list-models`, `--login`) are otherwise mutually exclusive; combining two is a usage error (64).
- **Seam wiring stays in the shim.** `--login` needs interactive seams (`BrowserLauncher` / `CodeReceiver` / `Pacer` / RNG) the data plane must never carry; `--list-models` needs the cache writer. So the **shim still chooses the wiring** — it now keys on the control flag instead of on `argv[0]`. Routing is defined **consistently with `parse_args`** (the one authoritative grammar): the shim asks the lib's `route(argv)` — a deep, narrow function built **on** `parse_args` returning `Route { Login, ListModels, Run }` — rather than reading the (private) `Flags` itself or hand-rolling an argument match. So the shim and the lib can never disagree (a value-taking flag whose value happens to look like a control flag, e.g. `--system=--login`, is correctly the value, not a route), AND the full `Flags`/`PartialConfig` surface stays private (a three-variant enum is the whole public addition, not the parser's internals). The shim is coverage-excluded; each lib entry (`run` / `login` / `list_models`) then re-parses authoritatively. A parse error (an unknown flag, two combined control ops) routes to `Run`, whose entry re-parses and surfaces the authoritative 64 — so `route` itself owns no error path.
- **Disambiguation is plain getopt — the caller's standard escapes, not a parser heuristic.** An argument is a control operation only by being a literal `--login` / `--list-models` in the option region; everything else is a prompt, and once the prompt region begins a `-`/`--` inside it is inert text, not an option. Two standard conventions close every ambiguity and leave the choice with the caller: **(1)** a bare `--` terminates option parsing — after it everything through EOF is the prompt (joined, §5.5), so `bz -- --login` is the prompt `--login` and `bz -- $UNTRUSTED` can never be read as a flag; **(2)** a flag value that begins with `-` uses the `--key=value` form (`--system=--login`), one argument, never read as a flag. So a process wiring **arbitrary or untrusted content** as the prompt sanitizes with `bz -- '<content>'` — the same idiom as `bl create -- "$TITLE"` — and the `--` guarantees the content can never be interpreted as a flag or control op. No bare word and no injected content is ever mistaken for control surface; the §5.10.1 namespace rule holds without the parser sniffing anything.

#### 5.10.2 `--raw` is symmetric (in **and** out); directional split deferred forward-compatibly

`--raw` is the single, knowingly-bent place where normalization is skipped (§5.4). Today it is **symmetric**: it skips *both* translators — `parse`+`encode` on the request (stdin bytes go to transport verbatim, the positional prompt is not used) and `decode`/normalize on the response (transport bytes stream back as `Event::Raw`). It is "brazen as a dumb authenticated pipe," not a parallel pipeline.

The owner's idea was a directional split — `--raw=out` / `--raw=in` for one-way rawness, bare `--raw` = both. All four input×output combinations are coherent and feasible (the request and response halves toggle independently in `serve`/`drive`), and the most compelling unidirectional case is **normalized-in / raw-out**: use `bz`'s request ergonomics (positional prompt, config-merged model/system/params, the model cache, auth) but capture the **exact provider wire bytes** — which is currently impossible, because raw-out forces raw-in (you must hand-write the entire provider-native request on stdin).

**Decision: ship symmetric-only at 0.1.0; do not split now.** The split is the rare CLI change here that is **not** a one-way door: bare `--raw` means "both" today and can keep meaning "both" forever, so adding `--raw=in` / `--raw=out` *later* is a pure, backward-compatible extension — no existing `--raw` invocation changes meaning. Because the door stays open for free, the parsimonious 0.1.0 move is to not pay the complexity (decoupling input-rawness from output-rawness is an internal refactor) for a debug-grade capability no current consumer is blocked on. We **document the limitation**: to get raw provider output you must also supply a raw provider request; `--json` carries the full response losslessly in canonical form for everyone else. `--raw=in`/`--raw=out` is a **sanctioned future extension**, kept alive by bare-`--raw`-means-both.

#### 5.10.3 The frozen surface (the full enumeration)

**Generation flags** (the flag layer of the config fold, §6.1; `--key value` and `--key=value` both accepted):

| Flag | Effect |
|------|--------|
| `--provider <id>` | provider row id (else routed from the model) |
| `--model <id>` | model id; a partial/absent id resolves against the cache (model-discovery §4) |
| `--api-key <key>` | inline credential (else the credential store / env) |
| `--system <text>` | leading system prompt (one `Content::Text`) |
| `--max-tokens <n>` · `--temperature <f>` · `--top-p <f>` | generation params |
| `--timeout-connect <s>` · `--timeout-response <s>` · `--timeout-idle <s>` | transport timeouts |
| `--stream` / `--no-stream` | stream the response (default) or fold one JSON body (tri-state, config §4.2) |
| `--thinking` | include reasoning/thinking output in text mode (§5.3) |

**Output mode** (one `OutMode`; the flags set the same field so a later one wins, e.g. `--json --text` ⇒ text):

| Flag | Mode |
|------|------|
| `--text` | default; human-readable text, with the tty-only pretty skin (§5.3, interactive-output.md) |
| `--json` | the full NDJSON canonical event stream (§5.2) |
| `--raw` | provider-native passthrough, symmetric in+out (§5.4, §5.10.2) |

**Input source flags:** `--input <file>` (read the request from a file instead of stdin — file and pipe die at `open()`, identical path, §5.5; `--input -` is **not** special-cased; missing/unreadable ⇒ 66) · `--config <file>` (use this config file instead of the search path; only changes *which* file the config layer reads — a direct flag still beats it, §6.3).

**Control short-circuit flags:** `--login` (+ `--browser`), `--list-models`, `--dump-config`, `--help`/`-h`, `--version`/`-V` — each replaces the data-plane run with a control action and exits (§5.10.1).

**Input channels** (§5.5): exactly one request source is used — a **positional prompt** if present (the operand tail; options-before-prompt, through-EOF, §5.5), **else** a canonical request (JSON) on **stdin** (or `--input FILE`, the simulated pipe). A present positional **wins and stdin is not read** — a positional plus a piped stdin silently **ignores** the pipe (the `head` idiom), **not** an error. An interactive-tty stdin with no positional is treated as **absent** (the shim's `isatty(0)`), so bare `bz` at a shell never blocks; it prints the usage to stderr and exits 64. Under `--raw` the stdin body is the verbatim provider request. Config inputs (env `BRAZEN_*` / provider-native vars, the config file) are the lower layers of the fold and are owned by config.md; flags override them.

**Exit codes:** the sysexits table of §8 — `0` (incl. refusal) / `64` usage / `66` no-input / `69` transport·4xx·premature-EOF / `70` 5xx / `77` auth / `78` config / `130`·`141`·`143` signals. **Frozen and coarse** (4xx incl. 429 → 69; retry policy rides `retryable`, not the code, §13.12).

#### 5.10.4 Migration (control verbs → flags)

The data plane is untouched; only the two verbs move. Before → after:

| Was (verb) | Is (flag) |
|------------|-----------|
| `bz login <provider> [--browser]` | `bz --login --provider <provider> [--browser]` |
| `bz list-models [--provider X] [--json]` | `bz --list-models [--provider X] [--json]` |
| `bz -- login` (prompt "login") | `bz login` (now just a prompt — no escape needed) |

Reconciliation scope for the implementation task (filed separately — this design note lands first, per the spec-precedence rule that architecture.md changes before its dependents):

- **Code.** `src/cli.rs` — add `login`/`list_models` bools (and recognize `--browser`) to `Flags`, parsed by `parse_args`; `src/main.rs` — key shim dispatch on the control flag (up to `--`) instead of `argv[0]`; `src/auth/login.rs` — drop `parse_login_args` and the per-verb `--help`/`--version` short-circuit, source the provider from `flags.config.provider`; `src/run/models.rs` — drop the `argv[1..]` verb-skip and its own short-circuit. The shared `HELP` text (`src/run/mod.rs`) moves the `VERBS:` block into the control-flag list. The `bz login` user-hint strings (the `NotLoggedIn`/`RefreshFailed` messages in `src/auth/refresh.rs`/`oauth.rs`, §7.1) become `bz --login --provider <id>`.
- **Specs.** model-discovery.md §2 (the verb framing + the "why a verb, not a flag" note — it defers to architecture.md by its own header) and auth.md §7.2 (`bz login` → `bz --login`) reconcile to this section. §13.5 below is amended.
- **User docs / scripts.** README.md (`bz login …`, `bz list-models …` examples) and `scripts/smoke.sh` (`bz login … --browser`).
- **Tests.** `tests/list_models*.rs`, `tests/login_*.rs`, `tests/oauth_smoke.rs` change `["list-models", …]` → `["--list-models", "--provider", …]` and `["login", <provider>, …]` → `["--login", "--provider", <provider>, …]`.

---

## 6. Config & Credentials (XDG, resolution, compiled config)

### 6.1 One schema, one fold, no privileged layer

There is exactly one config type, `PartialConfig`: every field `Option`, every provider entry sparse. Flags, env, file, and built-in defaults are **four instances of the same type**. Resolution is a fold under `Option::or` (highest-precedence operand on the left). No layer is privileged *in code*; precedence is the **order of operands**, which is data.

```rust
#[derive(Default, Clone, Debug, PartialEq)]   // custom Deserialize lives in config/partial_de.rs (the [[provider]] array ⇄ keyed-map seam)
pub struct PartialConfig {
    pub provider:         Option<String>,
    pub model:            Option<String>,
    pub api_key:          Option<Secret>,       // inline key => stateless, bypasses CredStore
    pub output:           Option<OutMode>,      // the enum is OutMode { Text | Ndjson | Raw }, NOT "OutputMode"
    pub thinking:         Option<bool>,         // --thinking: reasoning before the answer under the text projection (§5.3); inert outside it
    pub max_tokens:       Option<u32>,
    pub temperature:      Option<f32>,
    pub top_p:            Option<f32>,
    pub stream:           Option<bool>,
    pub timeout_connect:  Option<u64>,          // per-request transport timeouts, WHOLE SECONDS (config §4):
    pub timeout_response: Option<u64>,          //   connect / response-headers / inter-chunk idle bound
    pub timeout_idle:     Option<u64>,
    pub system:           Option<Vec<Content>>, // leading config/flag/file system prompt (§3.1, §4.4, Decision 10); filled when the request omits its own
    pub providers:        BTreeMap<String, PartialProvider>,  // merged sparsely, keyed by name
    pub extra:            Map<String, Value>,
}

// There is NO `resolve(flags, env, file, defaults, req)` wrapper. Resolution is two
// visible steps at the call site (run/mod.rs + config §3): an `Option::or` fold, then
// `into_resolved`. The request is NOT a fold operand — only its `model` is consulted,
// for routing — and the env layer is an already-projected `PartialConfig`.
let env_partial = partial_from_env(env)?;             // pure projection of an INJECTED snapshot → a PartialConfig layer
let merged = flags.or(env_partial).or(file).or(defaults());   // getConfigValue table: flag > env > config file > default. NOT a layer: the request.
let cfg: ResolvedConfig = merged.into_resolved(req_model.as_deref())?;  // routes the row (request.model wins, else config model), substitutes the alias once, validates → ConfigError = 78
// fill_absent(req, cfg): for each gen field, req.field = req.field.or(cfg.field); request-present fields untouched (§4.4)
```

The `fold` is the **same merge** for scalars and for the provider table, so the file can override one header on Anthropic without redeclaring the row. Built-in defaults are **not a bootstrap layer** — they are `include_str!("defaults.toml")` parsed through the same `toml::from_str::<PartialConfig>` path; "lowest precedence" = "last operand." A **missing config file is not an error**: it resolves to `PartialConfig::default()` (the identity element of the fold). No `--in-format`. A param a provider *requires* (e.g. Anthropic `max_tokens`) takes its sane default from that provider's row (`body_defaults`, config §4.1) as the **lowest-precedence operand**, so for a field the **request omits**, `getConfigValue` supplies it as **flag > env > config file > row default** (the request is *not* a fold operand — it is clean data, and `fill_absent` fills only what it leaves unset, §4.4); a param the API does not require stays `None` and is omitted — brazen never burdens the caller with a value the model needs, and never invents one the model doesn't.

### 6.2 The "compiled config file you point to"

"Compiling" is **not a build step and not a new verb.** A config file *is* a `PartialConfig` in TOML.

- **Author:** `bz --dump-config [flags…]` resolves the layers and prints the merged `PartialConfig` as TOML to stdout (secrets elided to an inert `"<redacted>"` sentinel — never a literal key, and never a `${VAR}` reference, because secrets live in the credential store or env and never in a dumped config, so no env-expansion mechanism is added). It is `serialize(merged_without_defaults)` — the same fold, no second path.
- **Use:** `bz --config prod.toml < req` loads that file as the *file layer*; because it is a full `PartialConfig` it can define provider rows, so it is a complete invocation with no other flags.

One schema, one (de)serializer; flags and file are the same fact in two encodings, and `--dump-config` is the only bridge. No `compile` subcommand (a new verb is a smell).

### 6.3 Config file location (a chicken-free fold)

```
--config FILE   >   $BRAZEN_CONFIG   >   $XDG_CONFIG_HOME/brazen/config.toml  (fallback ~/.config/brazen/config.toml)
```

### 6.4 Credentials — the ONE sanctioned stateful exception, XDG-correct

| Kind | Unix (`$XDG_*`) | macOS | Windows |
|---|---|---|---|
| Config (non-secret) | `$XDG_CONFIG_HOME/brazen/config.toml` | same (XDG, not `~/Library`) | same (XDG, not `%APPDATA%`) |
| Secrets (one file per provider) | `$XDG_DATA_HOME/brazen/credentials/<provider>.json` | `~/Library/Application Support/brazen/credentials/<provider>.json` | `%APPDATA%\brazen\credentials\<provider>.json` |

The **config** path is XDG on **all** platforms (`config_path`: `$XDG_CONFIG_HOME`, else `~/.config` — *not* `#[cfg]`-gated); only **secrets** follow the per-OS data dir above (`credentials_dir` *is* `#[cfg]`-gated: `$XDG_DATA_HOME` / `~/Library/Application Support` / `%APPDATA%`). Secret files are mode **0600** on Unix (enforced at `put`); Windows inherits the user-profile ACL — a **documented limitation**, not a code branch. One file per provider keeps the blast radius small and makes `bz --login` an atomic temp-file+rename write.

```rust
pub trait CredStore {
    fn get(&self, provider: &str) -> Option<Cred>;
    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()>;
}

#[derive(Serialize, Deserialize)]
pub enum Cred {
    ApiKey { key: Secret },
    Bearer { token: Secret },
    OAuth2 {
        access_token: Secret,
        refresh_token: Secret,
        expires_at: u64,
        #[serde(default)] scope: Option<String>,
        // a non-secret account id some providers bind to the cred and require echoed as
        // a header (OpenAI `ChatGPT-Account-ID`, auth §10.4); `None` for rows with none.
        #[serde(default)] account_id: Option<String>,
    },
}
```

Two methods only — no `is_valid`, `refresh`, `list`, `delete` in the data-plane trait (validity is *computed*; delete is control-plane). Single-source-of-truth applied to creds: **no `is_valid` flag** (freshness is the query `now + SKEW >= expires_at`); **`expires_at` is absolute** (computed once as `clock.now() + expires_in`; storing the relative value would be wrong the instant it's read back); **no `token_type` flag** (the `Cred` variant is the discriminant). The OAuth2 `scope` and `account_id` are `#[serde(default)]`, so a credential file written before either field existed still deserializes — the format grows **additively**, never breaking a stored cred. `Secret` is a newtype whose `Debug`/`Display` redact and whose `Serialize` writes plaintext only into the 0600 file — never into logs, `--dump-config`, or `provider_detail`.

### 6.5 The stateless-purity boundary — drawn at exactly one line

> **`Auth::apply` is the ONLY function in the data plane permitted to touch the credential store or the clock.**

Everything **before** `apply` (resolve, parse, encode) and everything **after** it returns (transport, decode, sink) is a **pure function of `(bytes_in, ResolvedConfig)`**. `apply`'s side-effecting authority is mediated by injected `CredStore` + `Clock`, so even *it* is pure relative to its injected deps. **The library never reads `std::env` (the env arrives as an injected `EnvSnapshot`), never calls `SystemTime::now` (the `Clock` seam), and never touches credentials except through `CredStore`.** It *does* perform two deterministic, injection-controlled file reads — `open_input` for `--input FILE` and `run`'s read of the resolved config path (`config_path(--config, env)` → `read_to_string`, a missing file folding to `PartialConfig::default()`). Both are reads of an *explicitly-named or env-derived* path with no hidden ambient input, so they stay 100%-testable from a tempfile and do not weaken the stateless boundary the §6.5 rule draws (which is about creds/clock/env-as-ambient-state, mediated by traits). Beyond `apply`'s creds + clock, `serve` reads the **model cache** through the injected `ModelCache` seam (model-discovery.md §5) — **read-only on the data plane** (its sole writer is the `list-models` verb), so it is pure relative to its injected dep exactly as `apply` is, and the "only `apply` touches the credential store or clock" rule is unbroken (the cache is neither). The genuinely impure surfaces — network, secret file, model-cache file, system clock, SIGPIPE — live only in the impls wired by `main()`.

The inline-key path (`--api-key` / `BRAZEN_API_KEY` / `ANTHROPIC_API_KEY`) **never constructs a `CredStore` at all** — it flows as `ResolvedConfig.inline_key`, projected onto `AuthCtx.inline_key` (§4.1) and preferred by `StaticSecretAuth::apply`, so a fully-stateless run touches zero files except stdin (and config, if pointed at one). The store is constructed lazily. The inline secret rides `AuthCtx`, **not** `ProviderCtx`, so it is unreachable from `Protocol::encode` — the boundary is enforced by the type, not merely by discipline.

---

## 7. Auth & SSO (browser launch, OAuth, refresh)

API-key, bearer, and OAuth2 are **one problem**: produce the finished auth headers on a `WireRequest`, given a store and a clock. Differences (where the secret comes from, whether it goes stale, what extra headers it implies) are internal to each impl; downstream is auth-blind.

```rust
struct StaticSecretAuth;  // api_key AND bearer: one impl, two ids; header NAME+scheme from Provider row data
struct OAuth2;            // endpoints/client_id/scopes are DATA on the auth row, read from AuthCtx.oauth (§4.1)
```

`api_key` and `bearer` differ **only** in `HeaderScheme` (`Raw` vs `Bearer`), which already lives on the row's `HeaderSpec` — so they are **one** `StaticSecretAuth` impl mapped from two `AuthId`s, not two structs (a second struct would be the redundant representation single-source-of-truth forbids; see auth.md §3). Both impls are **stateless unit structs** — every per-provider fact (the store key, the inline secret, the OAuth endpoints) arrives on `AuthCtx` (§4.1), so the registry shares one `&'static dyn Auth` per `AuthId` across every row.

- **`StaticSecretAuth::apply`** (behind `api_key` and `bearer`): secret = `auth.inline_key` if present, else `store.get(auth.store_key)`, else `Err(MissingCreds)` (→ 77). Sets the header named by `auth.api_header` using its `scheme` (`Raw` writes the value verbatim, `Bearer` prefixes `Bearer `), so `x-api-key: <key>` and `Authorization: Bearer <token>` are the same code modulo the row's scheme. Refresh is identity — the empty case of "refresh if stale," not a special case.
- **`OAuth2::apply`**: the only impl where staleness exists.

### 7.1 Silent refresh — the only stateful thing in a normal run

```rust
impl Auth for OAuth2 {
    fn apply(&self, req, ctx, auth, store, clock, tx) -> Result<(), AuthError> {
        let cfg = auth.oauth.ok_or(/* defensive; resolve guarantees Some, §4.1 */)?;
        let Some(Cred::OAuth2 { refresh_token, expires_at, .. }) = store.get(auth.store_key)
            else { return Err(AuthError::NotLoggedIn) };          // -> 77, tells user to `bz --login --provider <id>`
        let token = if is_expired(expires_at, clock.now()) {
            let wire  = build_token_exchange_request(cfg, Grant::Refresh(&refresh_token)); // pure
            let bytes = tx.send(wire)?.collect_to_end()?;          // the ONE impure seam
            let fresh = parse_token_response(&bytes, clock.now())?; // pure; sets ABSOLUTE expires_at
            store.put(auth.store_key, &fresh.as_cred(&refresh_token))?;  // persist for next process
            fresh.access_token
        } else { existing_access_token };
        req.set_header("authorization", &format!("Bearer {token}"));
        for (k, v) in &cfg.beta_headers { req.set_header(k, v); }  // e.g. anthropic-beta: oauth-2025-04-20
        Ok(())
    }
}
fn is_expired(expires_at: u64, now: u64) -> bool { now + SKEW >= expires_at }  // SKEW = 60s, a QUERY not a field
```

Detection is a pure comparison against the injected `Clock`; refresh reuses the Transport seam (mockable, offline-testable — no second network path); the new token is persisted so the next process starts fresh. **A failed refresh** (`invalid_grant`) → `RefreshFailed` → exit 77 with a message to `bz --login --provider <id>`. **Refresh never escalates to a browser** — that would block the data plane on interaction, which is forbidden.

### 7.2 First-time login — a separate control plane (`bz --login --provider <id>`)

> **Spelling (§13.13):** login is the `--login` control flag, not an `argv[0]` verb; its provider rides the existing `--provider`. The quarantine and flow logic below are unchanged — only the invocation spelling moved (`bz login <provider>` → `bz --login --provider <id>`). The `bz login` shorthands elsewhere in §7 are the pre-migration spelling, reconciled with the code by the §5.10.4 implementation task.

Interactive login is the **only interactive surface**, deliberately quarantined out of the data plane so `run` never blocks on a browser. It is a distinct verb whose entire job is to obtain a `Cred::OAuth2` and `CredStore::put` it. Two flows, selected by capability not vendor:

```rust
pub trait BrowserLauncher { fn open(&self, url: &str) -> io::Result<()>; }    // argv asserted as DATA when faked
pub trait CodeReceiver    { fn await_code(&self) -> io::Result<Callback>; }   // real = 127.0.0.1:0 listener
```

**(a) Device-code flow (RFC 8628) — default, headless-friendly:** POST device-authorization → print `user_code`+`verification_uri` to **stderr** → poll the token endpoint every `interval` s (default **5** if absent); on `authorization_pending` keep polling; on `slow_down` add 5 s cumulatively; stop+error if `device_code` expires (deadline via injected `Clock` — tests don't sleep).

**(b) Auth-code + loopback (RFC 8252) — `--browser`:** bind ephemeral port on the IPv4 loopback **literal `127.0.0.1:0`** (RFC 8252 §7.3 mandates the literal, *not* `localhost`, and any port — a real shipping-client interop bug) → build authorize URL with **PKCE** (`S256`) and `redirect_uri=http://127.0.0.1:<port>/callback` → `BrowserLauncher::open` → `CodeReceiver::await_code` captures `?code=&state=` → `parse_callback` validates `state` (CSRF) → token exchange → `parse_token_response` → `put`.

OAuth logic is the set of **pure functions** `build_authorize_url`, `parse_callback`, `build_token_exchange_request`, `parse_token_response`, `is_expired` — table-testable from literals. Auth-code, device-code, and refresh are three `Grant` inputs to the *same* token-exchange builder, not three paths. (Anthropic's specific `client_id`/scope are operator-supplied **data on the auth row**, never hard-coded vendor policy in the core.)

### 7.3 Browser launch — the only conditional compilation

```rust
fn browser_argv(url: &str) -> Vec<String> {   // PURE: returns argv, does not exec
    match std::env::consts::OS {
        "macos"   => vec!["open".into(), url.into()],
        "windows" => vec!["cmd".into(), "/C".into(), "start".into(), "".into(), url.into()],
        _         => vec!["xdg-open".into(), url.into()],
    }
}
```

Tested **as data** (assert the argv per OS); the real `Command::spawn(argv)` is the one logic-coverage-excluded line, alongside the socket bind.

---

## 8. Error Model

Every failure → `Event::Error(CanonicalError{ kind, message, provider_detail })` AND a POSIX exit code. Errors travel **in-band through the same Sink**, then exit is set — one path, no special "error mode." `retryable` and the exit code are **computed from `kind`**, never stored. **No `panic!`/`unwrap` on external input** (`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` on the data path). Provider-error *parsing* lives in each `Protocol::decode` (pure, tested without network). **Even under `--raw`, peek the HTTP status** so a raw 4xx/5xx never exits 0. For a **non-2xx handshake**, that peeked status is **carried on the whole-body `Frame` (`frame.status: Some(code)`, sse-decoder §9)** so `decode` derives `kind = ErrorKind::from_http_status(status)` from the authoritative value — `401|403 → Auth`, else `Provider{status}` (which already carries exit + `retryable`, so no second table). The body's `error.type`/`error.code` are diagnostics only and ride `provider_detail`; a `decode` must **never reconstruct the status from them** — the status has one home (the response) and is read, not guessed. Only a mid-stream in-band error (a 2xx stream, no governing status, CR-10) derives `kind` from the body.

Exit is computed (`from_kind` / `from_io`); last-error-wins; a signal supersedes everything.

| Code | Symbol | Class / `kind` | Trigger |
|-----:|--------|----------------|---------|
| **0** | `EX_OK` | success; also **Refusal** (`Finish{refusal}`, HTTP 200) | normal completion incl. provider refusal |
| **64** | `EX_USAGE` | `Usage` / `ParseInput` | bad flags, unknown flag, malformed stdin JSON |
| **66** | `EX_NOINPUT` | `Usage` (input-open) | `--input FILE` missing/unreadable |
| **69** | `EX_UNAVAILABLE` | `Transport`; `Provider` HTTP **4xx** (incl. 429) | connect/DNS/TLS/timeout, upstream client error, premature EOF |
| **70** | `EX_SOFTWARE` | `Provider` HTTP **5xx** | upstream server error/overloaded (`retryable=true`) |
| **77** | `EX_NOPERM` | `Auth` | 401/403, missing creds, OAuth refresh failure, login failure |
| **78** | `EX_CONFIG` | `Config` | no provider resolved, bad/contradictory config, unknown/ambiguous provider/model |
| **130** | — | `Interrupted` (SIGINT) | 128+2, by signal |
| **141** | — | `Interrupted` (SIGPIPE/BrokenPipe) | 128+13; Unix by signal, Windows by mapped write error |
| **143** | — | `Interrupted` (SIGTERM) | 128+15, by signal |

> **Table note (CR-10):** the `Provider` rows above are reached two ways — from the HTTP status of a failed handshake, **or** from an in-band mid-stream `Event::Error` whose `kind` was set by `decode`. For the in-band case the exit comes from the carried `kind` via `from_kind`; **the 2xx HTTP status of the streaming handshake is not consulted.**

**429 (rate limit) → 69**, distinguished by computed `retryable=true`, not a unique exit code (a new code would be a second home for "is it retryable"). This refines the skeleton's flat "all 4xx→69": 429 stays 69 and the meaning lives in `retryable`/`provider_detail`.

> **Decision — exit-code granularity (one-way-door review, RESOLVED: KEEP coarse, no split).** The owner asked whether to split 429 (or retryable provider errors generally) out of 69 into its own exit code. **No.** The exit code encodes the *sysexits failure class* — **where/what failed** (usage 64 / no-input 66 / transport·4xx 69 / 5xx 70 / auth 77 / config 78) — **not the retry policy.** `retryable` is an orthogonal query that cross-cuts that axis: transport is retryable but rides 69, a 400 is not retryable but also rides 69, a 5xx is retryable on 70. So exit 69 deliberately already conflates a retryable transport fault with a non-retryable 400 — the exit channel is *not* a retryable signal and was never meant to be. A split has only two shapes, both worse than the status quo:
> - **Single "retryable provider error" code** — re-homes the `retryable()` fact in the exit table (the exact second-home §3.5/§8 forbid; two reps drift). It also *fails its own use case*: the other retryable conditions (transport, 5xx) stay on 69/70, so a shell retry loop gating on the one new code still can't catch them all. Truly encoding retryable in the exit channel would mean collapsing transport+429+5xx into one code, destroying the transport-vs-client-vs-server distinction 69/70 give.
> - **Per-status fan-out** (429 its own code, then 503-vs-500 would demand the same) — sysexits is a fixed 64–78 vocabulary with no rate-limit entry, and this just mirrors HTTP status into exit codes, which `--json` already carries losslessly.
>
> Granularity already lives in the structured channel, strictly finer than any exit-code split could give: `bz --json` emits `{"error":{"kind":{"provider":{"status":429}},"message":…,"provider_detail":<raw body verbatim>}}`. A consumer branches on the exact `.kind.provider.status` (every status, not just 429) and reads `provider_detail` for hints an exit code can never carry (e.g. a `retry-after`). Structured discrimination belongs in the structured channel — `--json`/`provider_detail` (§5.2) — not in the coarse POSIX exit code.
>
> **Confirmed no shell consumer needs the split.** `bz` is single-shot — it never retries (no backoff anywhere in the data path); retry/backoff is the *caller's* job, and a caller orchestrating retries is already scripting (so `--json`+`jq` is available and they want `provider_detail` anyway). The repo's only shell consumer (`scripts/smoke.sh`) reads `$?` solely to assert expected codes, never to gate retry. `CanonicalError::retryable()` has no production caller — it is the canonical *home* of the fact, surfaced on the wire via the serialized status. If a pure-POSIX retry consumer ever materializes, the cheapest answer is still NOT a new exit code but an explicit opt-in flag — deferred, severable, built only on demand, and even then weighed against "just use `--json`."

**In-band mid-stream error → exit by `kind`, the 2xx HTTP status is NOT consulted (CR-10).** The table above is HTTP-status-driven, but Anthropic (and others) emit in-band SSE `error` events **after** the `200` handshake. An in-band `Event::Error` is produced by `decode` with **no governing HTTP status**, so its exit is set from its `CanonicalError.kind` via `ExitClass::from_kind` **directly** — never from a fabricated HTTP status. `decode` maps a mid-stream provider error to a `kind`: overloaded / 5xx-class → `Provider{status>=500}` → exit **70**; rate-limit → `Provider{status:429}` → exit **69**; otherwise `Transport` → exit **69** as the safe default. The successful `2xx` of the streaming handshake is not consulted for an in-band error — the `kind` carried on the event is the single source. **Last-error-wins** (a later in-band error overrides an earlier exit), and a **signal still supersedes** everything.

```rust
enum ExitClass { Ok, Usage, NoInput, Unavailable, Software, NoPerm, Config, Sig(i32) }
impl ExitClass {
    fn code(self) -> u8 { /* 0,64,66,69,70,77,78, or Sig(n) as u8 */ }
    fn from_kind(k: ErrorKind) -> ExitClass { /* pure, table-tested */ }
    fn from_io(e: &io::Error) -> ExitClass {
        match e.kind() { ErrorKind::BrokenPipe => ExitClass::Sig(141), _ => ExitClass::Unavailable }
    }
}
```

`code()` returns a numeric `u8`, not `process::ExitCode`: the numeric table is the single source of truth and is *directly* asserted (the opaque `ExitCode` has no `PartialEq`/accessor, so it can't be table-tested). The pure lib computes the `u8`; the `bz` shim materializes `process::ExitCode::from(code)` at the one process boundary (`main.rs`, the sole uncovered file). `from_kind(Interrupted)` defaults to `Sig(130)` (SIGINT); a live signal supersedes it via the signal path.

---

## 9. Testability & 100% Coverage

100% line coverage with **zero live network**, enforced by `cargo llvm-cov --fail-under-lines 100` (Makefile `make cov`, pre-commit hook) plus the 300-line/file rule.

### 9.1 The single seam, mocked

`trait Transport` is the only impure surface. `MockTransport` returns a fixed `status` + a `Vec<io::Result<Bytes>>` (which may contain an injected mid-stream `Err`) and optionally asserts the `WireRequest` (method/URL/headers/body — validating encode+auth end-to-end without network). A transport drop is just an `Err` element — the same `?` handles it as a clean read. **OAuth refresh reuses this seam** (no second mock).

### 9.2 Pure functions over fixtures (the bulk)

`parse`, every `encode`/`decode`, the `SseDecoder`/NDJSON line-framer, `resolve` (injected env snapshot), every `Sink`, the error→`CanonicalError`→exit-code mapping, and all OAuth URL/token builders+parsers are pure and table-tested from literals or golden captures.

**Golden SSE fixtures** (`tests/fixtures/<provider>.sse`), recorded from real streams, committed verbatim. v0.1 ships at minimum: `anthropic_messages_basic`, `anthropic_messages_thinking_tools` (carries a `signature`), `anthropic_messages_refusal` (HTTP 200 → `Finish{Refusal}`, exit 0), `anthropic_messages_pause`, `anthropic_error_overloaded` (HTTP 529 → exit 70), `openai_chat_basic`, `openai_chat_tools`, `openai_error_4xx`/401.

**The executable single-source-of-truth check:** `anthropic_messages_basic.sse` and `openai_chat_basic.sse` represent the *same logical response*; a property test asserts `normalize(decode_all(A)) == normalize(decode_all(O))`, where `normalize` drops only provider-inherent identity. Plus universal invariants over every fixture: decode ends in exactly one `End`; every `ContentDelta.index` has a preceding `ContentStart` and a following `ContentStop`; `Usage` fields are `Option`; the first event of any non-error stream is `MessageStart` carrying `v == 1`.

### 9.3 Deterministic streaming via adversarial rechunking

Every fixture is fed through a rechunker at hostile boundaries — `OneByte`, `MidData` (inside `data:`), `MidUtf8` (split a multi-byte sequence), `MidJsonNumber` (`"12"|"34"`), `WholeFixture` — and a parametric test asserts the decoded `Vec<Event>` is **identical across all strategies**. `MidUtf8` is what forces the `SseDecoder` to buffer a partial frame and partial UTF-8 tail (`Vec<u8>` until a blank-line terminator; `str::from_utf8` only complete frames). Mid-stream drop is `OneByte` + a trailing `Err`.

### 9.4 Browser/OAuth offline

`FakeBrowserLauncher` records argv as asserted data (never executes); `FakeCodeReceiver` returns a canned `?code=…`; the token exchange is `MockTransport::send`; `FakeClock` drives both fresh and stale branches with no time dependency. The real `browser_argv` is tested as data for all three OS values on one Linux runner. The loopback `CodeReceiver` is integration-tested in-process (real bind on `:0`, a test thread POSTs the code), so even the real receiver is exercised offline — only `main`'s OS-browser spawn line is uncovered.

### 9.5 Why 100% is real, not gamed

- **The only uncovered code is the `bz` shim** (`src/main.rs` + `src/native/` — the impure native wiring), excluded via `--ignore-filename-regex 'src/(main\.rs|native)'`; everything else (the library) is at 100%. `run` is exercised end-to-end with `MockTransport`. brazen ships as **one crate** (lib `brazen` + bin `bz`, so `cargo install brazen` builds the `bz` command), so the network-free invariant is no longer the crate graph's — `tests/purity.rs` enforces it instead, failing if any library module imports `ureq`/`libc`/`std::net` (§10, bl-c1e2, was bl-c420).
- **No `unwrap`/`panic` on the data path**, so there are no "impossible" arms to exclude — an unreachable arm is either dead code (delete it) or a missing test (add the fixture). Each forward-compat catch-all — `FinishReason::Other`, `Event::Other`, `ContentKind::Other`, `Delta::Other` — is covered by a deliberately-unknown fixture (`tests/canonical_event.rs`), proving the `v=1` no-error-on-unknown contract (§3.2) *executes* rather than merely being asserted in prose.
- The genuinely-unhittable rule is **reframe to remove the branch, not exclude it.**

### 9.6 stdin/`--input` parity & end-to-end `run`

One test feeds identical bytes through `Cursor<Vec<u8>>` and a `tempfile`, asserting byte-identical event streams (the executable proof that file-vs-pipe dies at `open()`). A second pair proves the **positional prompt** `bz "PROMPT"` and the equivalent stdin request build the same `CanonicalRequest`, and that a positional prompt *ignores* piped stdin (handing `read_request` a panics-on-read reader proves it is never touched — the pipe is the writer's concern, never an exit-64). **`fill_absent` + `getConfigValue`** are pure table tests: a field the request *sets* is returned untouched; a field it *omits* resolves **flag > env > config file > row-default**; and `--config FILE` only changes which file the config layer reads (a direct flag still beats it). The full `run` is called with a `Cursor` stdin, `Vec<u8>` stdout, fixture `MockTransport`, in-memory `CredStore`, and `FakeClock` — every mode (`--text` default, `--thinking`, `--json`/NDJSON, `--raw`, error-to-stderr-under-text, error-in-band-then-exit, refusal-exit-0, raw-4xx-exit-69) is one `run` invocation. **SIGPIPE** mapping is tested as the pure exit-code table (`signal_exit(SIGPIPE)==141`) plus one Unix integration test (`bz | head -c1` → real 141); the Windows path is covered on Linux via a `MockWriter` returning `BrokenPipe`.

### 9.7 Simulated-provider conformance (real transport, no real provider)

`MockTransport` ignores the request URL and the network, so a whole class of wire defects (the `ureq` round-trip itself, the non-2xx status peek, content-type handling) passes offline. The **live** suite (§10 below, README "Live conformance suite") closes that gap but needs real keys, so it is `#[ignore]`d and never runs in CI. The **simulated** tier (`tests/sim_conformance.rs` + `tests/sim_support/`, bl-7d5d) sits between them: a tiny loopback HTTP server (`FakeProvider`, `std::net` — no async, no new dep) replays the golden `tests/fixtures/*.sse`/`*.ndjson` captures, a temp `--config` points a provider row's `base_url` at it, and the **real `bz` binary** runs against it over the **real `HttpTransport`**. It asserts the normalized canonical grammar for all five providers (the surface that is identical across every provider) and that an HTTP `401` maps to exit `77` through the real status-peek — proving the end-to-end wire path with **no real provider and no key**. Not `#[ignore]`d, so it runs in plain `cargo test` (hence the CI matrix, on every platform). It is black-box (drives the bin, no lib linkage), so like the live suite it adds no library-coverage obligation.

### 9.8 Lib↔CLI interface parity (the published surface == the typed interface)

**The invariant.** The public `brazen` library surface is **exactly** the interface its entry points define — no more, no less. That interface is the **typed I/O**: a `CanonicalRequest` in, an `Event` stream out (the canonical model, §3, is the single source of truth), exposed through the `generate` entry point (§1) — plus the seams and config that drive it (`Host`, the traits, `ResolvedConfig`, …) and the two control verbs (`login`, `list_models`). The byte `bz` CLI is **one serialization** of exactly this. Every `pub` item is a 1.0 promise the moment a real release is cut, so the surface is held to that interface, not to whatever the test suite happened to need visible.

**The typed types ARE the interface (bl-b4a9 corrected bl-46e6).** bl-46e6 narrowed the surface to "what the `bz` binary *names*" — but `bz` is a thin byte-shim that pipes `stdin → run() → stdout` without ever naming `CanonicalRequest`/`Event`, so that oracle wrongly dropped the typed I/O. The canonical request and event vocabulary the wire format encodes ARE the interface, one serialization removed; exposing them is not widening the surface, it is *naming* it. So `generate` exists (the typed core, §1) and the request/event vocabulary is public. The §3.2 `v=1` forward-compat contract (`#[non_exhaustive]` + `Other` catch-alls) now protects both an embedder's `match` over the typed `Event` AND the wire — one contract, two encodings.

**Why the surface used to drift, and the fix.** Black-box integration tests in `tests/` can see only `pub` items, so unit-testing an *internal* (the pure `parse`/`pump`, the OAuth wire builders, `select_model`, the registry, every `Sink`) forced it `pub` — **test layout was driving the semver surface**. The fix: the unit/integration suite is **relocated in-crate** under `src/tests/` (declared `#[cfg(test)] mod tests;`), reaching non-interface internals through a `#[cfg(test)] pub(crate) use` **prelude** in `lib.rs` — invisible to `cargo public-api`/external consumers and stripped from every release build, so the tests stay ergonomic (`crate::Foo`) while the surface stays the interface. Modules are **private** (`mod foo`, never `pub mod`); the surface is declared **exclusively** by the crate-root `pub use` block. (`src/tests/` is the tests, not the lib-under-test — excluded from the coverage denominator like `src/native`, Makefile `cov`.)

**The invariant is a TYPE-CLOSURE, derived mechanically (no allowlist).** `tests/interface_parity.rs` parses the real sources with `syn`:

- **ROOTS** = every `pub` FN/CONST re-exported at the crate root (`run`, `generate`, `login`, `list_models`, `browser_argv`, `parse_ambient`, `query_from_request_line`, `EVENT_SCHEMA_VERSION`) — the entry points a consumer calls/reads.
- **CLOSURE** = every crate-defined TYPE transitively reachable from a root's signature, walking struct fields, enum variants, and trait-method signatures (so `generate`'s `CanonicalRequest`/`Event` pull in the whole vocab; `Host`'s traits pull in `WireRequest`/`Cred`/…).

The test asserts the set of `pub` TYPES at the crate root **== CLOSURE**, naming offenders on each side. Forward-compatible: a new entry point pulls its I/O types into CLOSURE automatically, with no per-capability edit. The two failure directions:

- **ORPHAN** (a `pub` type no entry point reaches): caught by this test (`types − CLOSURE`). Demote it to `pub(crate)`.
- **LEAK** (a private type in a public signature): caught by the **compiler** — `#![deny(private_interfaces, private_bounds)]` in `lib.rs` makes it a hard error. So the test polices orphans; the type system enforces no leak.

This is the same shape as `tests/purity.rs` (§9.5, §10): an invariant the crate graph can no longer enforce since lib+bin collapsed into one crate, re-established as an executable test. Internals the interface does not reach (`pump` — now the *production* byte adapter, §5.1; `Frame::as_str`; the OAuth builders) are `pub(crate)`/`#[cfg(test)]`-gated, neither published nor dead code in the release binary.

---

## 10. Portability

Target matrix (CI): **Linux / macOS / Windows × x86_64 / aarch64**, plus **`x86_64-unknown-linux-musl`** for the static-binary story. Six targets build **and test** on a native runner (portability proven by execution); the seventh, **`x86_64-apple-darwin`**, is **cross-built** on the Apple-Silicon runner (linking proven, not executed) because GitHub is sunsetting its only Intel-mac runner (`macos-13`, which stopped executing — cancelled every run). That gap is acceptable precisely because the native surface is deliberately tiny: the lib is pure portable Rust, and the one OS branch (browser argv) is tested as data on a runner that *does* execute. The matrix stays green because there is so little platform-specific code to break.

| Concern | Choice | Why it cross-compiles cleanly |
|---|---|---|
| TLS | `rustls` + `webpki-roots` | no OpenSSL/system lib, no `pkg-config`; `ring`'s crypto is vendored C/asm, statically linked; identical on musl/Windows/macOS |
| HTTP | minimal **blocking** client (`ureq`-class, rustls-backed) | fits the pure-`Iterator` pipeline; `into_reader()` streams chunk-by-chunk; no async runtime weight |
| Async runtime | **none** | blocking spine → no tokio, no async color; if ever justified it stays *behind* `Transport` |
| Paths/creds | `directories`/`etcetera` | `$XDG_*` (Unix), `%APPDATA%` (Win), `~/Library` (macOS) uniformly; 0600 on Unix; documented Windows-ACL limitation |
| Browser | one `match std::env::consts::OS` returning argv | the **only** conditional; behind `BrowserLauncher`; tested as data |
| Build | brazen: no build script, no C, no codegen | the only vendored C/asm is `ring`'s, compiled+statically linked — no system lib, no `pkg-config` to discover |

**SIGPIPE — one mechanism per OS** (§5.8): Unix `SIG_DFL`+die-by-signal; Windows `BrokenPipe`→mapped exit. Never both.

**Dependency surface — audited pre-ship (one-way-door, bl-2936).** `cargo machete`
finds no unused deps; the shipped `bz` binary carries no duplicated crate version
(the `getrandom` 0.2/0.4 split is dev-only, via `tempfile`). Feature sets are
already minimal: `serde`/`serde_json`/`toml` on defaults (no
`arbitrary_precision`/`raw_value`, no `preserve_order`); `ureq` is
`default-features = false, features = ["rustls"]` (bundles `webpki-roots` so a
static binary verifies certs with no system trust store; pulls neither
`native-roots` nor `platform-verifier`); `sha2` is `default-features = false` —
PKCE needs only the no_std `Sha256::digest`. `base64` stays a direct dep but costs
no extra crate (`ureq` pulls it transitively) and is confined to the one
`URL_SAFE_NO_PAD` engine. The `sha2` cluster — 7 RustCrypto crates for one 32-byte
hash — was weighed against hand-rolling SHA256 and against borrowing `ring`'s
(already linked): **kept**, because owning a crypto primitive, or moving PKCE
derivation off the pure golden-tested path into the shim, is the worse trade.

So the table's "no C" is shorthand to read precisely: brazen's *own* code has no
build script, no C, no codegen, and the graph pulls **no system C library** (no
OpenSSL, no `pkg-config`) — that is what keeps musl/cross clean. The one piece of
C/assembly in the graph is `ring`'s (rustls's crypto provider, reached only through
`ureq`), which its build script compiles and **statically vendors** — nothing is
discovered or linked from the system; a target C compiler is the only build-time
need.

**One crate, lib + bin (the balls→bl pattern):** brazen is a **single published crate**, so `cargo install brazen` builds the `bz` command — exactly how the `balls` crate ships `bl`. The pure pipeline + canonical types + the traits (`Protocol`, `Auth`, `Transport`, `CredStore`, `ModelCache`, `Clock`, `BrowserLauncher`, `CodeReceiver`) are the **library** (`[lib] name = "brazen"`, `src/lib.rs`). The **`bz` bin** (`[[bin]] name = "bz"`, `src/main.rs`) plus `src/native/` are the impure shim: they own the native impls (`HttpTransport` — the lone `ureq` user, XDG `CredStore`, XDG-cache `ModelCache`, `SystemClock`, `SystemBrowserLauncher`, the loopback `CodeReceiver`, the OS browser spawn) and are the only code allowed `ureq`/`libc`.

This used to be **two crates** (`brazen` lib + a separate `bz` bin crate), so the network-free invariant was the crate graph's — a lib module writing `use ureq` would not compile, because `ureq`/`libc` were not in the lib crate's deps (bl-c420). A single publishable crate that installs `bz` cannot keep that split (the bin must live in the `brazen` crate, so `ureq`/`libc` become crate-wide deps), so the invariant is re-established **as an executable test**: `tests/purity.rs` walks every library source file (everything under `src/` except `main.rs` and `native/`) and fails if it imports `ureq`/`libc`/`std::net` — a would-be impurity turns the build red there instead of at link time (bl-c1e2). The lib still reaches 100% coverage on a single runner: the hard-to-test native code is concentrated in `src/native/` + the bin and is coverage-excluded by path (§9.5).

---

## 11. Module Layout (respecting 300-line files)

The 300-line/code-file rule (`*.md`/`*.toml` exempt) is a forcing function toward narrow, deeply-tested modules. Each file below is comfortably under 300 lines.

```
lib (brazen) — src/
  lib.rs              crate attrs + the narrow public `pub use` surface + the
                      `#[cfg(test)]` internal prelude; modules are private (§9.8)
  run/
    mod.rs            the run() byte adapter: pre-sink (flags → config fold → input → sink), then `pump` generate's events
    generate.rs       the typed core: pub `generate(CanonicalRequest, ResolvedConfig, &Host) -> impl Iterator<Event>` (cache lookup → encode → auth → send)
    events.rs         response → lazy Iterator<Event>: errors carry the exit, frame→decode; whole_body / decode_full(non-stream) / StreamEvents
    serve.rs          serve_raw — the `--raw` byte passthrough (never decodes); exit seeded from the status (§5.4)
    models.rs         fetch_models — the ONE models-list GET, used only by the `list-models` verb (+ ListIo); writes the cache
  cli.rs              Args (injected argv+env+tty), parse_args -> Flags (flag layer + prompt/input/config/dump/help/version)
  canonical/
    request.rs        CanonicalRequest (model: empty==absent), Message, Content, Tool, ToolChoice, ImageSource, Role
    request_de.rs     custom serde for Content (bare-string ⇄ {"type":…}) — CR-4
    event.rs          Event, ContentKind, Delta, Usage, FinishReason (FinishReason hand-rolls flat serde)
    model.rs          Model + select_model (seed → wire id against the ordered list; "" → default)
    error.rs          CanonicalError, ErrorKind, ExitClass; retryable()/exit_code() (pure tables)
  pipeline/
    input.rs          open_input -> Box<dyn Read> (pipe == file); read_request (positional XOR stdin)
    parse.rs          parse() canonical-in
    sink.rs           Text / --thinking / NDJSON(--json) / --raw projections; the pump loop
    style.rs          Style::resolve(stdout_tty, env) — the pretty-skin activation predicate + every SGR/glyph (§5.3)
    pretty.rs         PrettySink — additive skin over TextSink: stdout byte-identical, human chrome on stderr (§5.3)
  config/
    mod.rs            the schema home: re-exports; doc of the one fold
    partial.rs        PartialConfig + PartialProvider + OutMode; the Option::or fold step
    partial_de.rs     custom Deserialize: the [[provider]] array-of-tables ⇄ keyed-map seam (§2.2)
    resolve.rs        into_resolved: route the row, substitute the alias, validate (body_defaults split)
    resolved.rs       ResolvedConfig + fill_absent + lead_with_preamble + strip_unsupported
    load.rs           parse_config / read_config_file / embedded defaults.toml
    env.rs            EnvSnapshot (injected env; the lib never reads std::env), partial_from_env, config_path
    dump.rs           --dump-config: serialize the merged-without-defaults PartialConfig to TOML; redact
    errors.rs         ConfigError set (78): NoProvider / AmbiguousModel / IncompleteProvider / BadValue
    provider.rs       Provider DATA record, ProtocolId/AuthId/HeaderSpec enums
  protocol/
    mod.rs            trait Protocol, ProviderCtx, WireRequest; re-exports Framing/Frame/DecodeState
    frame.rs          Frame, Framing, DecodeState, OpenBlock, Decoder seams
    sse.rs            shared SseDecoder + NdjsonDecoder + IdentityDecoder
    json.rs           leaf JSON accessors shared by every decode/encode (protocol-dedup D1)
    synth.rs          synthesized-stream mechanics for the structure-less decoders (D2)
    anthropic/        mod.rs + encode/{mod,blocks}.rs + decode/{mod,blocks,errors}.rs
    openai/           mod.rs + encode/{mod,messages}.rs + decode/{mod,blocks}.rs   (openai-chat)
    openai_responses/ mod.rs + encode.rs + decode/{mod,terminal}.rs               (ChatGPT/Codex)
    google_genai/     mod.rs + encode/{mod,contents}.rs + decode/{mod,blocks,errors}.rs
    ollama_chat/      mod.rs + encode.rs + decode/{mod,blocks,errors}.rs
  auth/
    mod.rs            trait Auth; StaticSecretAuth (ApiKey+Bearer), OAuth2Auth, NoAuth
    oauth.rs          OAuth2 apply
    wire.rs           pure OAuth wire builders (authorize url PKCE-S256, token exchange)
    refresh.rs        silent refresh — the only stateful thing in a normal run (uses clock+transport)
    flows.rs          the two `bz --login` flows (device-code + loopback)
    login.rs          `bz --login` — the quarantined control plane (LoginIo, Pacer, BrowserLauncher, CodeReceiver)
    jwt.rs            minimal UNVERIFIED JWT payload reads; urlencode.rs  form-urlencoded codec
  registry.rs         Registry::builtin() — protocol()/auth() total match on the closed key-enums
  transport.rs        trait Transport, TransportResponse, Timeouts, Bytes
  store.rs            trait CredStore, Cred, Secret; trait ModelCache; trait Clock; AmbientSpec/AmbientFormat
  os/
    browser.rs        browser_argv(os) -> argv  (the one cfg/OS-match)
  testing/            in-lib test doubles (`#[cfg(test)]`): clock.rs / store.rs / cache.rs / transport.rs / login.rs
  tests/              the relocated in-crate unit/integration suite (`#[cfg(test)] mod tests`,
                      §9.8): one module per former `tests/*.rs` + the shared `*_support` harness
data/
  defaults.toml       built-in provider table (include_str!) — config, exempt from the cap
bz bin — same crate, the impure shim (deps: ureq + libc; coverage-excluded) — src/
  main.rs             restore_sigpipe/isatty + wire the native seams + route the per-mode seams on the --login/--list-models control flag (§5.10.1), else run
  native.rs (+native/{creds,rng,cache,tests}.rs)  SystemClock, XdgCredStore, XdgModelCache, browser/loopback, OS RNG
  native/transport.rs HttpTransport — the lone `ureq` user, behind the lib's Transport seam
tests/                binary-driven black-box tests (sim/live conformance, smoke, the public-API
                      `ambient`); the executable invariants `purity.rs` (network-free) +
                      `interface_parity.rs` (lib↔CLI surface, §9.8); fixtures/ golden captures
```

A provider's `decode` that grows past 300 lines splits into `encode.rs`/`decode.rs`; the row in `provider.rs` is unaffected — severability holds (delete a provider = delete its module + its data row).

---

## 12. Deliberate tradeoffs (owned)

- **Blocking transport → one request per process**, no in-process fan-out (caller spawns N `bz`). Async would be a real refactor *behind* `Transport`, not a config change.
- **Canonical model is a lowest-meaningful-superset.** Provider-unique features ride `extra` in / `provider_detail`+`Raw` out, or require `--raw` (losing normalization). `--raw` is the one place "single representation" is knowingly bent.
- **Multi-turn / tool-loop / retry / backoff are the caller's job.** brazen exposes `retryable` but never acts on it.
- **Credentials are the sole stateful wart**; no secrets-backend abstraction (point env/config at an injected value).
- **No concurrent-refresh lock** — two `bz` processes could each refresh and double-`put`; last-write-wins on atomic temp-file rename is acceptable because either refreshed token is valid. A lock would be mechanism for a non-problem.

---

## 13. Resolved Decisions

The open questions are closed (owner-decided); recorded here for provenance.

1. **Per-row request-body defaults — sane defaults carried as provider-row data (`body_defaults`).** A row pins request-body fields it always needs in one `body_defaults` map (config §4.1), the lowest-precedence operand of the fold. A provider that *requires* `max_tokens` sets `body_defaults = { max_tokens = 4096 }` (anthropic), so the chain is **request value, else flag > env > config > row default** (the request is clean data; `getConfigValue` fills it only when the request omits it, §6.1). A param the API does not require, and the row does not pin, stays `None`/absent. No error path and no hard-coded constant — the defaults are tunable data (§3.1, §4.2, §6.1). This generalizes the former scalar `default_max_tokens` into one map so a row can also pin non-gen body knobs (`store`, `stream`) the canonical model does not field (config §4.1, auth §10.5).
2. **`--dump-config` redaction — inert sentinel.** Secrets dump as `"<redacted>"`, never a real key and never a `${VAR}` reference. No env-expansion mechanism is added; secrets live in the credential store or env, not in config (§6.2).
3. **OAuth — operator-configured.** Built-in provider rows are api-key/bearer only; OAuth `client_id`/scope are operator-supplied data on the auth row. No built-in OAuth row ships for v0.1 (Anthropic blocks third-party use of its OAuth tokens) (§4.2, §7).
4. **Windows secret-at-rest — documented limitation.** `0600` on Unix; the user-profile ACL on Windows, no DPAPI — accepted for v0.1 to keep the no-C-deps, single-binary portability story (§6.4, §10).
5. **`bz login` — a quarantined control operation, kept out of the data plane; not a sibling binary (§7.2).** ~~A `bz` subcommand/verb~~ — **superseded by §13.13**: it is the `--login` control-short-circuit flag (`bz --login --provider <id> [--browser]`), not an `argv[0]` verb. The quarantine stands (the one interactive surface, never entered by `run`); only its *spelling* changed from verb to flag (§5.10.1).
6. **Default output projection — `--text`.** `bz "what is 2+2"` → `4` with no flags; `--thinking` adds reasoning, `--json` is the full NDJSON event stream, `--raw` is passthrough. Human ergonomics is the default; harnesses opt into structure with `--json` (§5.1, §5.3).
7. **Bare prompt — the operand tail (through-EOF), positional argv sugar.** `bz <words…>` constructs a one-user-message `CanonicalRequest` from the **operand tail**: option parsing stops at the first non-option argument and **everything through EOF is the prompt** (operands joined by one space), so multi-word prompts need no quoting and any `-`/`--` after the prompt starts is inert text. The brick wall: **options must precede the prompt** (POSIX Guideline 9) — `bz --json "q"` selects JSON, `bz "q" --json` sends the prompt `"q --json"`; a leading-dash prompt uses `--`. A present positional **wins and stdin is not read** (the POSIX filter idiom — an unread pipe breaks upstream via `SIGPIPE`, like `head`), so there is no two-inputs error and no tty probe on that path; the positional is the explicit signal. It is a *constructor*, not a second request type. (Owner-decided "clear skies / brick walls", 2026-06: through-EOF + no post-prompt options **supersedes** the prior single-positional rule that errored on a second operand and "never a silent join", and the older "both → exit 64" draft. §2, §5.5, §5.10.1.)
8. **The pipe is clean request data, not a config layer.** A field the request sets is used as-is; a field it omits is supplied by `getConfigValue` = **flag → env → config file → app/row default** (`--config` only sets *which* file; a direct flag still beats that file). Per field the order is **request > flag > env > config > default**, expressed as two mechanisms (the request, then config-fills-the-rest) — an invoker never learns a body-vs-flag precedence protocol. Supersedes the earlier "body is a fold operand" draft (§4.3, §4.4, §6.1, §6.3).
9. **Event-schema version — `MessageStart.v` (currently `1`).** The single handshake harnesses pin to; a backward-incompatible `Event` change bumps it. First field of the first event on every non-`--raw`/non-error stream (§3.2). **What is additive WITHIN a `v`:** the vocabulary only grows — a consumer MUST ignore an unknown event `type`/content `kind`/`delta` and unknown object fields, so adding a new event/kind/delta/field does **not** bump `v`; only a removal, rename, or semantic change does. The types enforce this (`#[non_exhaustive]` + an `Other` catch-all on every open enum, §3.2). The error event carries no `v` (an error-first stream has no `MessageStart`), so the `CanonicalError`/`ErrorKind` wire schema cannot be version-gated — instead it is made forward-tolerant the same way: `ErrorKind` is `#[non_exhaustive]` and carries an `Other` catch-all, so it too grows *additively* (an unrecognized snake_case `kind` decodes to `Other`, never errors). The escape hatch must ship before the 0.1.0 freeze because a pinned binary cannot be made tolerant after the fact; only a removal/rename/semantic change is forbidden (§3.2, §3.3).
10. **System prompt — `req.system` and `Role::System` are distinct facts, both kept.** `req.system` = the leading config/flag/file-sourced prompt (the ergonomic path); `Role::System` = a positional in-band system message a transcript carries. Adapters project both deterministically — no dedup, no drift; not collapsed to one home (§3.1).
11. **Auth-private data rides `AuthCtx`, a second projection — not `ProviderCtx`.** `Auth::apply` needs the credential-store key and (for OAuth) the auth-row endpoints; `ProviderCtx` withholds both because it is *also* handed to `Protocol::encode`. A dedicated `AuthCtx { store_key, inline_key, oauth }` reaches only `apply`, so a live credential is **type-level unreachable** from the protocol layer — making §6.5's "only `apply` touches credentials" an invariant the compiler enforces. `store_key` is an opaque `CredStore` key (never matched), `oauth` is `Some` iff `AuthId::OAuth2` (a resolve invariant, else 78), so all three `Auth` impls stay stateless `&'static` unit structs. Resolves the `ProviderCtx`-carries-no-name vs. `apply`-needs-the-store-key tension surfaced by the auth spec (§4.1, §6.5, §7).
12. **Exit-code granularity — KEEP coarse (4xx incl. 429 → 69); do NOT split.** The exit code encodes the *sysexits failure class* (where/what failed), not retry policy; `retryable` is an orthogonal computed query surfaced at full per-status granularity in `--json`/`provider_detail`. A split would either re-home the `retryable` fact in the exit table (the second-home §3.5/§8 forbid) or fan exit codes out per HTTP status (which `--json` already carries losslessly). Confirmed no shell consumer needs it: `bz` is single-shot and never retries, retry is the caller's job, and the repo's only consumer (`scripts/smoke.sh`) reads `$?` only to assert codes. If one ever does, the answer is an explicit opt-in flag, not a new code (§8).
13. **Control operations are flags, not verbs (one-way-door review, RESOLVED).** `bz login` / `bz list-models` move to the control-short-circuit flags `bz --login` / `bz --list-models` (login's provider via the existing `--provider`, not a positional). Pre-0.1.0 the shim dispatched on `argv[0]`, so `login`/`list-models` were verbs sharing the prompt's slot — `bz login` could never be the prompt `"login"`, and every future verb would permanently shrink the bare-prompt set (shipping `bz models` later silently breaks `bz "models"`). The fix dissolves the special-case `argv[0]` dispatch into the **existing** control-flag path (`--help`/`--version`/`--dump-config` are already exactly this — distinct modes, own output, no request body, expressed as flags), so the surface gains no new *category*, only two new flags. The bare-prompt namespace becomes total and **frozen — it can never shrink again** (the §5.10.1 rule: a leading bare word is always a prompt; control ops are always flags). The data plane is untouched; the shim still picks the per-mode seam wiring, keying on the flag instead of `argv[0]`. Supersedes decision §13.5 and model-discovery.md §2's "why a verb, not a flag" (§5.10.1, §5.10.4).
14. **`--raw` stays symmetric at 0.1.0; the directional split is deferred forward-compatibly (one-way-door review, RESOLVED).** `--raw` skips both translators (verbatim request in, verbatim response out). The owner's `--raw=in`/`--raw=out` split is feasible and the normalized-in/raw-out case is genuinely useful (capture the exact provider wire from an ergonomic prompt — currently impossible), but it is the **one CLI change here that is not a one-way door**: bare `--raw` means "both" today and forever, so `--raw=in`/`--raw=out` can be added *later* with zero breakage. So we do not pay the decoupling complexity now for a debug-grade need no consumer is blocked on; we document the limitation (raw-out requires raw-in; `--json` is the lossless canonical alternative) and keep the extension sanctioned (§5.4, §5.10.2).

---

## 14. Roadmap of follow-on specs

This spec is the contract; the follow-on specs derive from it and must not contradict it (if one needs to, this spec changes first). They are named, not numbered — git is the log. The active work roadmap — these specs plus the ordered v0.1 implementation slice — is tracked in `bl`.

- **The OpenAI chat mapping** (`openai-chat-mapping.md`) — Canonical ⇄ OpenAI chat/completions.
- **The Anthropic messages mapping** (`anthropic-messages.md`) — Canonical ⇄ Anthropic messages.
- **The auth spec** (`auth.md`) — Auth, OAuth/SSO & the credential store.
- **The config spec** (`config.md`) — config schema, resolution & compiled config.
- **The SSE-decoder spec** (`sse-decoder.md`) — SSE / NDJSON decoder & `DecodeState`.
- **The providers spec** (`providers.md`) — provider rows: Mistral, OpenAI responses, Google generative-ai, Ollama.