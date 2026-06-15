# Spec 0001 — Architecture & I/O Contract

> **Status:** accepted · **Owner:** orionriver · **Supersedes:** none
> **Living document.** Edited like code. Per-protocol/-provider/-auth specs derive from this one and must not contradict it; if they need to, this spec changes first.

---

## 1. Purpose & Scope

`brazen` (binary `bz`) is **one small, stateless Rust binary** that adapts every LLM provider (OpenAI, Anthropic, Mistral, Google, local Ollama, …) and every wire protocol (OpenAI `chat/completions`, OpenAI `responses`, Anthropic `messages`, Google `generative-ai`) behind a single pipe contract:

```
stdin (canonical request) → bz → stdout (canonical event stream, streamed until one End token)
```

It is a **low-level building block for agents**, not an agent. It does exactly one network round-trip per process, normalizes the provider's stream into one canonical event vocabulary, and exits with a POSIX-correct code. It handles all auth models (API key, bearer, OAuth/SSO with browser launch). It is published as a crate so the pure pipeline can be embedded directly.

This spec is the authoritative **architecture and I/O contract**: the spine, the canonical model, the adapter abstraction, the I/O/streaming/POSIX behavior, config/credentials/auth, the error model, and the testability/portability constraints. It is decisive: where a choice exists, this document makes it.

### The spine (the whole binary in one signature)

```rust
fn run(
    args:      Args,
    stdin:     &mut dyn Read,
    stdout:    &mut dyn Write,
    transport: &dyn Transport,
    store:     &dyn CredStore,
    clock:     &dyn Clock,
) -> ExitCode
```

`main()` is a ~5-line shim that wires the real impls (`HttpTransport`, XDG `CredStore`, `SystemClock`) and calls `run`. **`main` is the only uncovered surface in the codebase**; everything testable lives behind `run`. The pipeline is `Iterator<Item = io::Result<Bytes>>` end to end — **blocking, never async**, no tokio, no `impl Stream`, no lifetime-parameterized stream types. A blocking, rustls-backed HTTP client streams chunk-by-chunk via `into_reader()`, so the pipeline is genuinely incremental without async color.

---

## 2. Non-Goals

- **Not an agent.** No multi-turn loop, no tool-execution loop, no retry/backoff. brazen *exposes* `retryable` but never acts on it; the caller orchestrates.
- **Not stateful** beyond the one sanctioned exception (XDG config + credential/token storage). No history, no cache, no session files.
- **No in-process fan-out.** One request per process (blocking transport). A caller that wants N concurrent requests spawns N `bz`.
- **No input-dialect auto-detection.** Input is canonical-by-default. No structural sniffing, no `--in-format`. `--raw` on input means "these bytes are already provider-native."
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
    pub max_tokens: Option<u32>,        // None; a provider row's default_max_tokens fills it at lowest precedence (§4.2), omitted when None and not required
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop: Vec<String>,              // empty = no stop sequences
    pub stream: bool,                   // request-shaping only; NOT how we detect stream-over (that's Event::End)
    #[serde(flatten)]
    pub extra: Map<String, Value>,      // adaptive thinking, reasoning_effort, safetySettings, … (the long-tail valve only)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Message { pub role: Role, pub content: Vec<Content> }  // ALWAYS a vec; a bare string decodes to vec![Text(..)]

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role { System, User, Assistant, Tool }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text(String),
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: Vec<Content>, is_error: bool },
    Thinking { text: String, signature: Option<String> },  // signature is LOAD-BEARING
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
- **`Thinking.signature` is `Option<String>` and must round-trip verbatim.** Anthropic thinking blocks carry an opaque `signature`; the API rejects modified/absent signatures on multi-turn replay. brazen is stateless, but the **caller round-trips** thinking blocks across turns through brazen, so the canonical model must carry the signature unmodified or it destroys the caller's ability to continue. `None` covers providers without the concept (the empty-set rule). Adapters never fabricate a signature — copy through or leave `None`.
- **`ToolChoice` is a typed enum, not an `extra` knob** — all providers express the same four intents under different spellings ("lift known knobs explicitly").

### 3.2 The canonical streaming Response (the Event taxonomy)

**Output is a STREAM, never a struct.** A non-stream provider response is the *folded* stream — the same `Vec<Event>`, produced in one decode call. We never store the response twice. The non-stream and streaming `decode` emit the *same* vocabulary.

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    MessageStart { id: Option<String>, model: Option<String>, role: Role },
    ContentStart { index: u32, kind: ContentKind },
    ContentDelta { index: u32, delta: Delta },
    ContentStop  { index: u32 },
    Usage(Usage),
    Finish { reason: FinishReason },
    Error(CanonicalError),
    Raw(Vec<u8>),   // only under --raw
    End,            // THE provider-agnostic terminator
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentKind { Text, ToolUse { id: String, name: String }, Thinking }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Delta {
    TextDelta(String),
    JsonDelta(String),       // tool-call argument fragments (string, NOT a parsed Value)
    ThinkingDelta(String),
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub input: Option<u32>,
    pub output: Option<u32>,
    pub cache_read: Option<u32>,
    pub cache_write: Option<u32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FinishReason {
    Stop,                                                   // end_turn / "stop" / STOP / done
    Length,                                                 // max_tokens / "length" / MAX_TOKENS
    ToolUse,                                                // tool_use / "tool_calls"
    StopSequence,                                           // stop_sequence
    Refusal { category: String, explanation: Option<String> },  // arrives as HTTP 200, exit 0
    Pause,                                                  // Anthropic pause_turn (resumable agentic flow)
    Other(String),                                          // unknown reason — caught, never panics
}
```

- **`ContentStart` and `ContentDelta` are deliberately separate** — block-open is not folded into the first delta. Anthropic streams `content_block_start` (carrying tool id/name *before* any argument bytes); OpenAI reveals `tool_calls[i].id`+`.function.name` on the first chunk. Keeping them separate lets the OpenAI adapter *synthesize* a `ContentStart{ToolUse{id,name}}` the first time an index appears, so **identity always precedes content for every block on every provider** — the consumer never needs a "did I see the id yet?" branch.
- **`Usage` fields are `Option`, never fabricated `0`.** A provider that never reports `cache_read` leaves it `None`; `0` would be a lie ("zero cache hits" vs "unknown"). Cumulative; emitted whenever a provider reveals it.
- **Refusal is a `Finish`, NEVER an `Error`.** A refusal arrives as HTTP 200 with `stop_reason:"refusal"`. Modeling it as an error would invent a second representation of "the request succeeded" and force a non-zero exit on a 200. `category` is `String` (open, growing set per the API) and `Other(String)` defends the top-level reason field — neither panics on an unknown value.

### 3.3 Error — its own event, `retryable` computed

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CanonicalError {
    pub kind: ErrorKind,
    pub message: String,
    pub provider_detail: Option<Value>,   // parsed upstream error body, verbatim
    // NOTE: no `retryable` field — it is computed.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind { Usage, ParseInput, Config, Auth, Provider { status: u16 }, Transport, Interrupted }

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
| "stream is over" | **computed** — arrival of `Event::End` | one terminator; never a `bool done` |
| "is this a non-stream response" | **computed** — fold the event stream | response stored once, as the stream |
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
/// Owns the wire dialect. Pure: no IO, no clock, no creds.
pub trait Protocol: Send + Sync {
    fn encode(&self, req: &CanonicalRequest, ctx: &ProviderCtx) -> Result<WireRequest, Error>;
    /// Consume ONE already-parsed frame -> zero or more canonical events.
    /// Statefulness (open-block indices, cumulative usage) is caller-owned `DecodeState`,
    /// so the impl is a pure fn of (frame, state) and shareable as &'static dyn.
    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, Error>;
    /// Which transport framing this protocol uses. DATA, not behaviour.
    fn framing(&self) -> Framing;   // Sse | Ndjson | Identity
}

/// The ONLY consumer of CredStore. The stateless boundary is drawn exactly here.
pub trait Auth: Send + Sync {
    fn apply(
        &self,
        wire: &mut WireRequest,
        store: &dyn CredStore,
        clock: &dyn Clock,
        transport: &dyn Transport,   // for silent OAuth refresh — same seam, no new IO surface
        ctx: &ProviderCtx,
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
    pub api_header: &'a HeaderSpec,              // x-api-key | Authorization:Bearer | x-goog-api-key — DATA
    pub beta_headers: &'a [(&'a str, &'a str)],  // provider-level STATIC betas (e.g. anthropic-version)
    pub extra: &'a Map<String, Value>,           // the severability valve (passthrough knobs)
}
```

`ProviderCtx` carries **no `name`, no `ProtocolId`, no `AuthId`** — by the time encode/decode/auth run, the vendor identity has been spent on the registry lookup. The impls are vendor-blind; they see only capabilities.

### 4.2 Provider is DATA, not a trait

```rust
#[derive(Deserialize, Clone)]
pub struct Provider {
    pub name: String,            // table key only — never matched on in the pipeline
    pub base_url: String,
    pub protocol: ProtocolId,    // OpenAiChat | AnthropicMessages | (later) OpenAiResponses | GoogleGenAi | OllamaChat
    pub auth: AuthId,            // ApiKey | Bearer | OAuth2
    pub api_header: HeaderSpec,  // { name:"x-api-key", scheme:Raw } | { name:"Authorization", scheme:Bearer }
    #[serde(default)] pub beta_headers: Vec<(String, String)>,
    #[serde(default)] pub model_aliases: Map<String, String>,  // alias -> wire model id (computed lookup)
    #[serde(default)] pub default_max_tokens: Option<u32>,     // sane default for a param THIS provider requires; lowest-precedence operand in the fold
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
default_max_tokens = 4096          # Anthropic requires max_tokens; brazen supplies a sane default (override via config/flag)

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

There is **no model→provider routing table** (a second home would drift). Resolution is a **query over the rows**, computed once during config resolution: the user names a provider explicitly (`--provider anthropic`) **or** brazen finds the single row whose `model_aliases` contains the model. Two matches is a `Config` error (78), never a silent pick — ambiguity is surfaced. Alias→wire-id substitution happens **in resolution**, so `ProviderCtx.model` is already the wire id and `encode` has no model logic.

### 4.4 Dispatch with NO match-on-provider

```rust
pub struct Registry {
    protocols: HashMap<ProtocolId, &'static dyn Protocol>,
    auths:     HashMap<AuthId,     &'static dyn Auth>,
}
impl Registry {
    pub fn builtin() -> Self {
        let mut protocols: HashMap<_, &'static dyn Protocol> = HashMap::new();
        protocols.insert(ProtocolId::OpenAiChat,        &OpenAiChat);
        protocols.insert(ProtocolId::AnthropicMessages, &AnthropicMessages);
        // adding a protocol = ONE insert + ONE enum arm + ONE module. Nothing else.
        let mut auths: HashMap<_, &'static dyn Auth> = HashMap::new();
        auths.insert(AuthId::ApiKey, &ApiKeyAuth);
        auths.insert(AuthId::Bearer, &BearerAuth);
        auths.insert(AuthId::OAuth2, &OAuth2Auth);
        Self { protocols, auths }
    }
}
```

The data flow through `run` — **no vendor name appears**:

```rust
let cfg   = resolve(flags, env, file, BUILTIN_TOML)?;   // -> ResolvedConfig { provider, model, raw, … }
let canon = parse(reader)?;                             // canonical-by-default; --raw skips this
let proto = registry.protocols[&cfg.provider.protocol]; // lookup, not match
let auth  = registry.auths[&cfg.provider.auth];         // lookup, not match
let ctx   = ProviderCtx::from(&cfg);

let mut wire = if cfg.raw { WireRequest::raw(canon.bytes) } else { proto.encode(&canon, &ctx)? };
auth.apply(&mut wire, store, clock, transport, &ctx)?;  // the one cred seam
let resp = transport.send(wire)?;                       // the one IO seam
let mut exit = exit_from_status(resp.status, cfg.raw);  // raw 4xx/5xx still exits non-zero

let framing = if cfg.raw { Framing::Identity } else { proto.framing() };
let mut decoder = framing.decoder();
let mut state   = DecodeState::default();
for chunk in resp.body {
    for frame in decoder.push(chunk?)? {
        let events = if cfg.raw { vec![Event::Raw(frame.into_bytes())] } else { proto.decode(frame, &mut state)? };
        for ev in events { sink.write(&ev)?; }          // flushed per event
    }
}
sink.write(&Event::End)?;
exit
```

The only enums the core touches are **map keys**; the only `match` in the path is `if cfg.raw` — a *mode*, not a vendor. Exactly one place knows specific providers: `Registry::builtin()`, the severable seam itself.

### 4.5 Auth-mode-dependent headers live on the Auth impl, not the row

The Anthropic `anthropic-beta: oauth-2025-04-20` header differs **by auth mode on the same provider** (api-key vs OAuth on `api.anthropic.com`). A per-provider-only field cannot express "this header only under OAuth" without a core branch. So:

- **Provider row** carries auth-mode-*independent* headers (`anthropic-version`) — always sent.
- **`OAuth2Auth::apply`** adds `Authorization: Bearer …` **and** `anthropic-beta: oauth-2025-04-20`, and performs the silent refresh. OAuth knowledge is fully contained in one `Auth` impl.

### 4.6 Severability proof (the grading rubric)

- **Add Mistral** (new provider, existing protocol+auth): **one `[[provider]]` row, zero Rust.** Delete the row → gone.
- **Add OpenAI "responses"** (new dialect): `mod openai_responses` (`impl Protocol`, pure, fixture-tested) + one `ProtocolId` arm + one `Registry::builtin()` insert. **Nothing in `run`, `resolve`, `parse`, the Sink, the canonical model, or the other Protocol impls changes** — `response.completed` normalizes to the same `Event::End`. Delete module+arm+insert → gone; rows that referenced it fail at resolve with a `Config` error.
- **Add Google's `x-goog-api-key`**: already expressible as `HeaderSpec { name:"x-goog-api-key", scheme:Raw }` on the row; `ApiKeyAuth` reads `ctx.api_header` by data — no branch, no new impl.

The invariant that holds it all: **the core's only knowledge of a provider is `cfg.provider.protocol` / `cfg.provider.auth` as map keys.** `name` never reaches a dispatch site.

---

## 5. I/O & Streaming & POSIX Contract

brazen is a **strict unix filter**: deterministic, line-oriented, unbuffered-per-event, signal-correct.

### 5.1 The single output seam

```rust
/// The one output surface. `write` is called once per Event, in order.
/// Implementors MUST flush before returning — no event is buffered across calls.
trait Sink { fn write(&mut self, ev: &Event) -> io::Result<()>; }
enum OutMode { Ndjson, Text, Raw }   // default = Ndjson
```

The driver loop is mode-agnostic and is the only place exit state is computed; `Event::Error` does **not** stop the loop (errors are in-band; partial-response-then-error is representable).

### 5.2 stdout framing — NDJSON (default)

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

Sample wire shape:

```
{"type":"message_start","id":"msg_01…","model":"claude-3-5-sonnet","role":"assistant"}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}
{"type":"content_stop","index":0}
{"type":"usage","input":12,"output":2,"cache_read":null,"cache_write":null}
{"type":"finish","reason":"stop"}
{"type":"end"}
```

### 5.3 `--text` projection

Human/REPL mode: emit only `ContentDelta::TextDelta` bytes, concatenated, no framing, no injected trailing newline. Thinking/json/tool/usage/start events drop. `Finish`/`Error`/`End` produce no stdout bytes (they still set the exit code). Flush per delta. Terminator is **EOF on stdout** (an in-band `end` line would corrupt human output) — one of the two modes where `Event::End` is not the on-wire terminator.

### 5.4 `--raw` passthrough

The single, knowingly-bent place where normalization is skipped:

- **Decode is identity.** Transport bytes become `Event::Raw(Bytes)` chunks; `RawSink` writes them verbatim, flushing per chunk.
- **The provider's own terminator stands.** brazen does **not** append `{"type":"end"}`.
- **`--raw` is symmetric on input**: stdin bytes are already provider-native and go to transport verbatim (no `parse`, no `encode`). The encode/auth/transport middle is byte-identical to the normalized path — raw is "skip the two translators," not a parallel pipeline.
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

### 5.6 Termination / the end token

- **NDJSON: the end token is the literal line `{"type":"end"}`**, emitted exactly once, last, after any `Finish`/`Usage`/`Error`. **`Finish` ≠ end**: `Finish` is *why* generation stopped; `End` means *the byte stream is over*. A refusal is `Finish{refusal}` + `End`, exit 0. A consumer reads lines until `type == "end"`, then expects EOF.
- **Premature upstream EOF** → an in-band `Event::Error{kind:Transport}`, then `Event::End`, then exit 69. **An NDJSON stream always ends with `end`, even on failure** — one invariant dissolves the "did it finish?" edge case.
- `--text`/`--raw` terminate by **EOF on stdout**.

### 5.7 Flushing & backpressure

Flush after every event — no `BufWriter` accumulation. Backpressure is the kernel's pipe buffer honored by blocking writes: `write_all` blocks when the downstream pipe is full, and because the pipeline is a blocking `Iterator`, we don't pull the next transport chunk until the current event is flushed. No internal queue, no dropped events, no unbounded memory. This is *why* the blocking spine is correct: backpressure is free and end-to-end. We never set stdout nonblocking.

### 5.8 Signals — one mechanism per OS (mutually exclusive)

- **Unix: restore `SIGPIPE` to `SIG_DFL` at startup.** Rust defaults to `SIG_IGN`; we undo it in the `main` shim (one `unsafe libc::signal` call). A write to a closed stdout then kills the process by signal — like `cat | head` — exit **141** (128+13). We never reach a `BrokenPipe` write-error branch.
- **Windows: no SIGPIPE.** `write_all`/`flush` returns `BrokenPipe`; `pump` maps it to the same exit **141**. The only place the path differs.
- **SIGINT → 130, SIGTERM → 143** by default disposition — we install no handlers (nothing stateful to unwind; creds are written synchronously inside `Auth::apply` before any streaming). Already-flushed NDJSON lines stay valid. Determinism via *absence* of mechanism.

```rust
#[cfg(unix)]    unsafe fn restore_sigpipe() { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
#[cfg(not(unix))] fn restore_sigpipe() {}
```

### 5.9 stderr

Silent on the happy path. stderr carries **only** a fatal condition that prevents the stream from starting *and* cannot be in-band — practically: flag/usage parse failure (exit 64) and input-open failure (exit 66), both **before any Sink exists**. Once the Sink exists, all errors are in-band `Event::Error` on stdout — a given failure appears in exactly one place.

---

## 6. Config & Credentials (XDG, resolution, compiled config)

### 6.1 One schema, one fold, no privileged layer

There is exactly one config type, `PartialConfig`: every field `Option`, every provider entry sparse. Flags, env, file, and built-in defaults are **four instances of the same type**. Resolution is a fold under `Option::or` (highest-precedence operand on the left). No layer is privileged *in code*; precedence is the **order of operands**, which is data.

```rust
#[derive(Default, Deserialize)]
pub struct PartialConfig {
    pub provider:    Option<String>,
    pub model:       Option<String>,
    pub api_key:     Option<Secret>,        // inline key => stateless, bypasses CredStore
    pub output:      Option<OutputMode>,    // Ndjson | Text | Raw
    pub max_tokens:  Option<u32>,
    pub temperature: Option<f32>,
    pub stream:      Option<bool>,
    pub providers:   BTreeMap<String, PartialProvider>,  // merged sparsely, keyed by name
    pub extra:       Map<String, Value>,
}

pub fn resolve(flags: PartialConfig, env: &EnvSnapshot, file: PartialConfig, defaults: PartialConfig)
    -> Result<ResolvedConfig, ConfigError>
{
    let env = partial_from_env(env);                       // pure projection of an INJECTED snapshot
    let merged = flags.or(env).or(file).or(defaults);      // precedence IS this order
    merged.into_resolved()                                 // discharge Options; error if provider/model unset
}
```

The `fold` is the **same merge** for scalars and for the provider table, so the file can override one header on Anthropic without redeclaring the row. Built-in defaults are **not a bootstrap layer** — they are `include_str!("defaults.toml")` parsed through the same `toml::from_str::<PartialConfig>` path; "lowest precedence" = "last operand." A **missing config file is not an error**: it resolves to `PartialConfig::default()` (the identity element of the fold). No `--in-format`. A param a provider *requires* (e.g. Anthropic `max_tokens`) takes its sane default from that provider's row (`default_max_tokens`) as the **lowest-precedence operand**, so the chain is exactly **flag > config > row default**; a param the API does not require stays `None` and is omitted — brazen never burdens the caller with a value the model needs, and never invents one the model doesn't.

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
| Config (non-secret) | `$XDG_CONFIG_HOME/brazen/config.toml` | `~/Library/Application Support/brazen/config.toml` | `%APPDATA%\brazen\config.toml` |
| Secrets (one file per provider) | `$XDG_DATA_HOME/brazen/credentials/<provider>.json` | `~/Library/Application Support/brazen/credentials/<provider>.json` | `%APPDATA%\brazen\credentials\<provider>.json` |

Secret files are mode **0600** on Unix (enforced at `put`); Windows inherits the user-profile ACL — a **documented limitation**, not a code branch. One file per provider keeps the blast radius small and makes `bz login` an atomic temp-file+rename write.

```rust
pub trait CredStore {
    fn get(&self, provider: &str) -> Option<Cred>;
    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()>;
}

#[derive(Serialize, Deserialize)]
pub enum Cred {
    ApiKey { key: Secret },
    Bearer { token: Secret },
    OAuth2 { access_token: Secret, refresh_token: Secret, expires_at: u64, scope: Option<String> },
}
```

Two methods only — no `is_valid`, `refresh`, `list`, `delete` in the data-plane trait (validity is *computed*; delete is control-plane). Single-source-of-truth applied to creds: **no `is_valid` flag** (freshness is the query `now + SKEW >= expires_at`); **`expires_at` is absolute** (computed once as `clock.now() + expires_in`; storing the relative value would be wrong the instant it's read back); **no `token_type` flag** (the `Cred` variant is the discriminant). `Secret` is a newtype whose `Debug`/`Display` redact and whose `Serialize` writes plaintext only into the 0600 file — never into logs, `--dump-config`, or `provider_detail`.

### 6.5 The stateless-purity boundary — drawn at exactly one line

> **`Auth::apply` is the ONLY function in the data plane permitted to touch the credential store or the clock.**

Everything **before** `apply` (resolve, parse, encode) and everything **after** it returns (transport, decode, sink) is a **pure function of `(bytes_in, ResolvedConfig)`**. `apply`'s side-effecting authority is mediated by injected `CredStore` + `Clock`, so even *it* is pure relative to its injected deps. **Nothing in the library reads `std::env`, opens `$XDG_*`, or calls `SystemTime::now`** — those impurities live only in the three injected impls wired by `main()`.

The inline-key path (`--api-key` / `BRAZEN_API_KEY` / `ANTHROPIC_API_KEY`) **never constructs a `CredStore` at all** — it flows as `ResolvedConfig.inline_key`, which `ApiKey::apply` prefers, so a fully-stateless run touches zero files except stdin (and config, if pointed at one). The store is constructed lazily.

---

## 7. Auth & SSO (browser launch, OAuth, refresh)

API-key, bearer, and OAuth2 are **one problem**: produce the finished auth headers on a `WireRequest`, given a store and a clock. Differences (where the secret comes from, whether it goes stale, what extra headers it implies) are internal to each impl; downstream is auth-blind.

```rust
struct ApiKey;                  // header NAME from Provider row data
struct Bearer;
struct OAuth2 { cfg: OAuthConfig }   // endpoints/client_id/scopes are DATA on the auth row
```

- **`ApiKey::apply`**: secret = `inline_key` if present, else `store.get(provider)`, else `Err(MissingCreds)` (→ 77). Sets the header named by `ctx.api_header` (data, not a vendor branch). Refresh is identity — the empty case of "refresh if stale," not a special case.
- **`Bearer::apply`**: same shape, `Authorization: Bearer <token>`.
- **`OAuth2::apply`**: the only impl where staleness exists.

### 7.1 Silent refresh — the only stateful thing in a normal run

```rust
impl Auth for OAuth2 {
    fn apply(&self, req, store, clock, tx, ctx) -> Result<(), AuthError> {
        let Some(Cred::OAuth2 { refresh_token, expires_at, .. }) = store.get(&self.cfg.provider)
            else { return Err(AuthError::NotLoggedIn) };          // -> 77, tells user to `bz login`
        let token = if is_expired(expires_at, clock.now()) {
            let wire  = build_token_exchange_request(&self.cfg, Grant::Refresh(&refresh_token)); // pure
            let bytes = tx.send(wire)?.collect_to_end()?;          // the ONE impure seam
            let fresh = parse_token_response(&bytes, clock.now())?; // pure; sets ABSOLUTE expires_at
            store.put(&self.cfg.provider, &fresh.as_cred(&refresh_token))?;  // persist for next process
            fresh.access_token
        } else { existing_access_token };
        req.set_header("authorization", &format!("Bearer {token}"));
        for (k, v) in &self.cfg.beta_headers { req.set_header(k, v); }  // e.g. anthropic-beta: oauth-2025-04-20
        Ok(())
    }
}
fn is_expired(expires_at: u64, now: u64) -> bool { now + SKEW >= expires_at }  // SKEW = 60s, a QUERY not a field
```

Detection is a pure comparison against the injected `Clock`; refresh reuses the Transport seam (mockable, offline-testable — no second network path); the new token is persisted so the next process starts fresh. **A failed refresh** (`invalid_grant`) → `RefreshFailed` → exit 77 with a message to `bz login`. **Refresh never escalates to a browser** — that would block the data plane on interaction, which is forbidden.

### 7.2 First-time login — a separate control plane (`bz login <provider>`)

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

Every failure → `Event::Error(CanonicalError{ kind, message, provider_detail })` AND a POSIX exit code. Errors travel **in-band through the same Sink**, then exit is set — one path, no special "error mode." `retryable` and the exit code are **computed from `kind`**, never stored. **No `panic!`/`unwrap` on external input** (`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` on the data path). Provider-error *parsing* lives in each `Protocol::decode` (pure, tested without network). **Even under `--raw`, peek the HTTP status** so a raw 4xx/5xx never exits 0.

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

**429 (rate limit) → 69**, distinguished by computed `retryable=true`, not a unique exit code (a new code would be a second home for "is it retryable"). This refines the skeleton's flat "all 4xx→69": 429 stays 69 and the meaning lives in `retryable`/`provider_detail`.

```rust
enum ExitClass { Ok, Usage, NoInput, Unavailable, Software, NoPerm, Config, Sig(i32) }
impl ExitClass {
    fn code(self) -> ExitCode { /* 0,64,66,69,70,77,78, or Sig(n) */ }
    fn from_kind(k: ErrorKind) -> ExitClass { /* pure, table-tested */ }
    fn from_io(e: &io::Error) -> ExitClass {
        match e.kind() { ErrorKind::BrokenPipe => ExitClass::Sig(141), _ => ExitClass::Unavailable }
    }
}
```

---

## 9. Testability & 100% Coverage

100% line coverage with **zero live network**, enforced by `cargo llvm-cov --fail-under-lines 100` (Makefile `make cov`, pre-commit hook) plus the 300-line/file rule.

### 9.1 The single seam, mocked

`trait Transport` is the only impure surface. `MockTransport` returns a fixed `status` + a `Vec<io::Result<Bytes>>` (which may contain an injected mid-stream `Err`) and optionally asserts the `WireRequest` (method/URL/headers/body — validating encode+auth end-to-end without network). A transport drop is just an `Err` element — the same `?` handles it as a clean read. **OAuth refresh reuses this seam** (no second mock).

### 9.2 Pure functions over fixtures (the bulk)

`parse`, every `encode`/`decode`, the `SseDecoder`/NDJSON line-framer, `resolve` (injected env snapshot), every `Sink`, the error→`CanonicalError`→exit-code mapping, and all OAuth URL/token builders+parsers are pure and table-tested from literals or golden captures.

**Golden SSE fixtures** (`tests/fixtures/<provider>.sse`), recorded from real streams, committed verbatim. v0.1 ships at minimum: `anthropic_messages_basic`, `anthropic_messages_thinking_tools` (carries a `signature`), `anthropic_messages_refusal` (HTTP 200 → `Finish{Refusal}`, exit 0), `anthropic_messages_pause`, `anthropic_error_overloaded` (HTTP 529 → exit 70), `openai_chat_basic`, `openai_chat_tools`, `openai_error_4xx`/401.

**The executable single-source-of-truth check:** `anthropic_messages_basic.sse` and `openai_chat_basic.sse` represent the *same logical response*; a property test asserts `normalize(decode_all(A)) == normalize(decode_all(O))`, where `normalize` drops only provider-inherent identity. Plus universal invariants over every fixture: decode ends in exactly one `End`; every `ContentDelta.index` has a preceding `ContentStart` and a following `ContentStop`; `Usage` fields are `Option`.

### 9.3 Deterministic streaming via adversarial rechunking

Every fixture is fed through a rechunker at hostile boundaries — `OneByte`, `MidData` (inside `data:`), `MidUtf8` (split a multi-byte sequence), `MidJsonNumber` (`"12"|"34"`), `WholeFixture` — and a parametric test asserts the decoded `Vec<Event>` is **identical across all strategies**. `MidUtf8` is what forces the `SseDecoder` to buffer a partial frame and partial UTF-8 tail (`Vec<u8>` until a blank-line terminator; `str::from_utf8` only complete frames). Mid-stream drop is `OneByte` + a trailing `Err`.

### 9.4 Browser/OAuth offline

`FakeBrowserLauncher` records argv as asserted data (never executes); `FakeCodeReceiver` returns a canned `?code=…`; the token exchange is `MockTransport::send`; `FakeClock` drives both fresh and stale branches with no time dependency. The real `browser_argv` is tested as data for all three OS values on one Linux runner. The loopback `CodeReceiver` is integration-tested in-process (real bind on `:0`, a test thread POSTs the code), so even the real receiver is exercised offline — only `main`'s OS-browser spawn line is uncovered.

### 9.5 Why 100% is real, not gamed

- **The only uncovered file is `src/bin/main.rs`** (the ~5-line shim), excluded via `#[coverage(off)]` or `--ignore-filename-regex`. `run` is exercised end-to-end with `MockTransport`.
- **No `unwrap`/`panic` on the data path**, so there are no "impossible" arms to exclude — an unreachable arm is either dead code (delete it) or a missing test (add the fixture). `Finish::Other`/`FinishReason::Other` are covered by a deliberately-bogus fixture, proving the no-panic-on-unknown contract *executes*.
- The genuinely-unhittable rule is **reframe to remove the branch, not exclude it.**

### 9.6 stdin/`--input` parity & end-to-end `run`

One test feeds identical bytes through `Cursor<Vec<u8>>` and a `tempfile`, asserting byte-identical event streams (the executable proof that file-vs-pipe dies at `open()`). The full `run` is called with a `Cursor` stdin, `Vec<u8>` stdout, fixture `MockTransport`, in-memory `CredStore`, and `FakeClock` — every mode (NDJSON, `--text`, `--raw`, error-in-band-then-exit, refusal-exit-0, raw-4xx-exit-69) is one `run` invocation. **SIGPIPE** mapping is tested as the pure exit-code table (`signal_exit(SIGPIPE)==141`) plus one Unix integration test (`bz | head -c1` → real 141); the Windows path is covered on Linux via a `MockWriter` returning `BrokenPipe`.

---

## 10. Portability

Target matrix (CI): **Linux / macOS / Windows × x86_64 / aarch64**, plus **`x86_64-unknown-linux-musl`** for the static-binary story. The matrix stays green because the native surface is deliberately tiny.

| Concern | Choice | Why it cross-compiles cleanly |
|---|---|---|
| TLS | `rustls` + `webpki-roots` | pure-Rust, no OpenSSL/system lib; no `pkg-config`; identical on musl/Windows/macOS |
| HTTP | minimal **blocking** client (`ureq`-class, rustls-backed) | fits the pure-`Iterator` pipeline; `into_reader()` streams chunk-by-chunk; no async runtime weight |
| Async runtime | **none** | blocking spine → no tokio, no async color; if ever justified it stays *behind* `Transport` |
| Paths/creds | `directories`/`etcetera` | `$XDG_*` (Unix), `%APPDATA%` (Win), `~/Library` (macOS) uniformly; 0600 on Unix; documented Windows-ACL limitation |
| Browser | one `match std::env::consts::OS` returning argv | the **only** conditional; behind `BrowserLauncher`; tested as data |
| Build | no build scripts, no C deps, no codegen | nothing to break per-target; pure `cargo build` |

**SIGPIPE — one mechanism per OS** (§5.8): Unix `SIG_DFL`+die-by-signal; Windows `BrokenPipe`→mapped exit. Never both.

**Crate split:** the pure pipeline + canonical types + the traits (`Protocol`, `Auth`, `Transport`, `CredStore`, `Clock`, `BrowserLauncher`, `CodeReceiver`) are the **`lib` (`brazen`)**, with no platform-specific deps in the lib's own code (all behind trait injection). `bz` is the thin **`bin`** owning the native impls (`HttpTransport`, XDG `CredStore`, `SystemClock`, `SystemBrowserLauncher`, the loopback `CodeReceiver`, the OS browser spawn). This is why the lib reaches 100% on a single runner: the hard-to-test native code is concentrated in `bin` and is minimal.

---

## 11. Module Layout (respecting 300-line files)

The 300-line/code-file rule (`*.md`/`*.toml` exempt) is a forcing function toward narrow, deeply-tested modules. Each file below is comfortably under 300 lines.

```
lib (brazen)
  lib.rs              re-exports; the run() spine
  canonical/
    request.rs        CanonicalRequest, Message, Content, Tool, ToolChoice, ImageSource
    event.rs          Event, ContentKind, Delta, Usage, FinishReason
    error.rs          CanonicalError, ErrorKind; retryable()/exit_code() (pure tables)
  pipeline/
    input.rs          open_input -> Box<dyn Read> (pipe == file)
    parse.rs          parse() canonical-in
    sink.rs           NDJSON / --text / --raw projections; the pump loop
  config/
    resolve.rs        4-layer PartialConfig fold + embedded defaults.toml; --dump-config
    provider.rs       Provider DATA record, ProtocolId/AuthId enums, builtin table
  protocol/
    mod.rs            trait Protocol, ProviderCtx, WireRequest, Framing, Frame, DecodeState
    sse.rs            shared SseDecoder + NDJSON line-framer
    openai_chat.rs    encode + decode
    anthropic.rs      encode + decode (the verified wire shape)
  auth/
    mod.rs            trait Auth; ApiKey / Bearer
    oauth.rs          OAuth2 apply + the pure URL/token builders + is_expired
  registry.rs         Registry::builtin()
  transport.rs        trait Transport, TransportResponse
  store.rs            trait CredStore, Cred, Secret; trait Clock
  os/
    browser.rs        browser_argv(os) -> argv  (the one cfg/OS-match)
data
  defaults.toml       built-in provider table (include_str!) — config, exempt from the cap
bin (bz)
  main.rs             ~5-line shim: wire real impls, restore_sigpipe, call run  (the only uncovered file)
tests
  fixtures/<provider>.sse   golden captures
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

1. **Default `max_tokens` — a sane default carried as provider-row data.** A provider that requires the param declares `default_max_tokens` on its row (`anthropic = 4096`), and that value is the lowest-precedence operand in the fold, so the override chain is **flag > config > row default**. A param the API does not require stays `None` and is omitted. No error path and no hard-coded constant — the default is tunable data (§3.1, §4.2, §6.1).
2. **`--dump-config` redaction — inert sentinel.** Secrets dump as `"<redacted>"`, never a real key and never a `${VAR}` reference. No env-expansion mechanism is added; secrets live in the credential store or env, not in config (§6.2).
3. **OAuth — operator-configured.** Built-in provider rows are api-key/bearer only; OAuth `client_id`/scope are operator-supplied data on the auth row. No built-in OAuth row ships for v0.1 (Anthropic blocks third-party use of its OAuth tokens) (§4.2, §7).
4. **Windows secret-at-rest — documented limitation.** `0600` on Unix; the user-profile ACL on Windows, no DPAPI — accepted for v0.1 to keep the no-C-deps, single-binary portability story (§6.4, §10).
5. **`bz login` — a `bz` subcommand.** The one quarantined interactive verb, kept out of the data plane; not a sibling binary (§7.2).

---

## 14. Roadmap of follow-on specs

This spec is the contract; the follow-on specs derive from it and must not contradict it (if one needs to, this spec changes first). The active work roadmap — these specs plus the ordered v0.1 implementation slice — is tracked in `bl`.

- **0002 — Canonical ⇄ OpenAI chat/completions mapping.**
- **0003 — Canonical ⇄ Anthropic messages mapping.**
- **0004 — Auth, OAuth/SSO & the credential store.**
- **0005 — Config schema, resolution & compiled config.**
- **0006 — SSE / NDJSON decoder & DecodeState.**
- **0007 — Provider rows: Mistral, OpenAI responses, Google generative-ai, Ollama.**