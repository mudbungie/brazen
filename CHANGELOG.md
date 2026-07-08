# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is hand-authored: it is the deliberate, human-readable record of what
each release ships. `release-plz` prepends future versions above the entries
below — see the "Releasing" section of the README.

## [Unreleased]

### Fixed

- **SSE decoder robustness — three defects (bl-b8a0).**
  - **Non-SSE 200 body is diagnosed, not discarded.** A `200` that selects the
    streaming path but whose body is not SSE (a gateway HTML page, a JSON error
    served with `200`) frames zero frames, so `terminated` stays false and the
    premature-EOF path fired a **bare** `Transport`/69 while throwing away the
    upstream error text. Now, when the stream drains with `!terminated` **and the
    framer emitted zero frames**, the accumulated body head (bounded, 8 KiB) rides
    the error's `provider_detail` — parsed JSON verbatim when it parses, else the
    bytes as a string — the streaming sibling of the non-2xx path's verbatim body
    preservation. A stream that framed ≥1 frame keeps the bare error (its content
    already surfaced). "Frames ever decoded" is a `run`-driver-local fact, not
    stored on the framer or on `DecodeState`.
  - **Leading UTF-8 BOM stripped (WHATWG SSE).** A stream-start `EF BB BF` was
    never stripped; it corrupted the first field name, and in the OpenAI dialect
    (first block is a bare `data:`) dropped the ENTIRE first frame. Now stripped
    once at stream start, split-safe under one-byte rechunking. Later `EF BB BF`
    bytes are ordinary data.
  - **`find_frame_end` no longer rescans from 0 (O(n²) → O(n)).** The blank-line
    search now resumes from a remembered offset (backing up 3 bytes for a
    `\r\n\r\n` straddling a chunk boundary), so a frame that never terminates is
    scanned once as it arrives, not re-scanned from the front on every push. Pure
    performance change, byte-identical framing (the rechunking determinism suite is
    the witness). The frame buffer is deliberately **left uncapped** — a cap would
    spuriously fail a legitimate giant frame; the residual never-terminating-frame
    memory exposure (the idle timeout never trips while bytes flow) is documented
    honestly in sse-decoder.md §6.2.
  - Specs: sse-decoder.md §6.1/§6.2/§9.1, architecture.md §5.6.

### Added

- **`--base-url <url>` / `BRAZEN_BASE_URL` host override (bl-1f9e)** — point a run
  at a custom endpoint (local proxy, mock server, vLLM, tenant gateway) with **no
  temp config file**, the flagship embedding-harness case. ONE more top-level scalar
  in the existing fold (flag > env > file), lifted onto the RESOLVED provider row's
  `base_url` at resolve exactly as `--model` overrides the routing model — full
  precedence **flag > env > file-scalar > row**. It does **not** create a row:
  protocol, auth, and routing/alias substitution stay the resolved row's (the common
  case — *same provider, different host*), so it lands before row completion and the
  lifted row is still validated whole. Distinct from a `[[provider]]` row's own
  `base_url` (the two keys never collide; a file may carry both, each round-tripping
  `--dump-config` independently). Applies uniformly through the one `into_resolved`
  fold, so it reaches generation, `--list-models`, `--count-tokens`, and `--login`
  alike; `--dump-config` shows the merged scalar. **Explicitly declines full row
  injection** — no `--protocol`/`--auth` flags: a genuinely new provider is
  config-file territory (a `[[provider]]` row), and reconstructing a row scalar-by-
  scalar on the CLI is the new-flag smell the frozen surface keeps shut. Specs:
  config.md §3.4/§4.5/§8, architecture.md §5.10.3.
- **`bz --count-tokens` control op (bl-24e5)** — provider-accurate input-token
  counting for harness callers (lernie enforces per-role token budgets on
  estimates today). A fifth control short-circuit flag (§5.10.1 family, never a
  verb): reads a canonical request the SAME way the data plane does (positional
  prompt XOR stdin/`--input`, plus `-f` attachments), resolves provider/model
  identically (model seed placed against the per-provider cache), does ONE
  round-trip to the provider's count endpoint, and emits `{"input_tokens": N}`
  under `--json` else the bare number `N`. No retry, no cache write. Mutually
  exclusive with `--login`/`--list-models`/`--dump-config` (combination → 64);
  `--help`/`--version` probes still win. Endpoint knowledge is DATA on the
  protocol (`Protocol::count_tokens`, sibling of `models_shape()`), reusing each
  dialect's own `encode` projection: **Anthropic** (`POST
  /v1/messages/count_tokens`, key `input_tokens`) and **Google**
  (`models/{model}:countTokens`, key `totalTokens`) are live; OpenAI-chat,
  OpenAI-responses, and Ollama have no count endpoint and **DECLINE** with a
  `Config` error (exit 78) — a fabricated estimate is a lie, so the caller's own
  estimate stays its fallback. Specs: architecture.md §5.10.1/§8,
  anthropic-messages.md §2.11, providers.md §10.1.
- **`Retry-After` carried on `CanonicalError` (`retry_after_seconds: Option<u32>`)**
  — a caller-owned retry loop pacing a 429/529 now gets the provider's authoritative
  pacing hint, an HTTP **response header** the parsed error `provider_detail` (the
  body) never holds. Populated only from the non-2xx handshake header, in whole
  seconds; both wire forms parse — a bare `delay-seconds` integer, and an `HTTP-date`
  (IMF-fixdate) whose delay is `date - now` against the injected `Clock` seam (never a
  wall-clock read; obsolete rfc850/asctime date forms are a documented narrowing).
  `None` where the header is absent/unparseable (empty-set rule) and inherently on a
  mid-stream 2xx-stream error. The header is captured on `TransportResponse.retry_after`
  (the one impure seam) and stamped onto the whole-body error in `run` (the sibling of
  the 404-hint enrichment), never on the `Frame` and never in the clockless
  `from_http_status`. Additive under the `v=1` grows-only tolerance:
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, so old error lines stay
  byte-identical and a `v=1` consumer already ignores the unknown key — no
  `EVENT_SCHEMA_VERSION` bump. `MockTransport::with_retry_after` mirrors the seam for
  tests. Specs: architecture.md §3.3/§8/§11, sse-decoder.md §9, providers.md.
- **Provider-reported model metadata in `--list-models`** — `Model` gains three
  additive, `Option`-shaped fields lifted from the provider's OWN list GET (no new
  flags, no second round-trip): `context_window` (input token limit),
  `max_output_tokens` (output limit), `display_name`. Google's `models.list`
  serves all three (`inputTokenLimit`/`outputTokenLimit`/`displayName`), Anthropic
  only `display_name`; OpenAI/Ollama serve none, so those stay `None` — the
  empty-set rule (a harness derives what a provider serves, hand-configures only
  what it does not). CARRIED, never fabricated: absent metadata is `None` (the
  Usage zero-vs-unknown principle). The `--json` object `{"models":[…]}` gains the
  optional keys (omitted when unreported via `skip_serializing_if`); text mode
  (ids one per line) is UNCHANGED. The per-provider cache schema extends
  grows-only — a cache an older `bz` wrote (id + default only) reads clean to
  `None`, no version bump. The `ModelsShape` DATA table grows a `ModelKeys`
  projection (the metadata key paths per protocol) and the `[provider.models]`
  row override may NAME them (e.g. `context_key = "context_window"` lifts the
  Codex slug shape's own field). Specs: model-discovery §2/§3/§3.1/§3.2/§5.1/§8,
  config §4.4. [bl-1421]
- **First-class Anthropic server-tool support (CR-4 resolved)** — opaque
  `ServerToolUse`/`ServerToolResult` passthrough on request replay and response
  decode (the open-set `*_tool_result` family round-trips by tag suffix with zero
  per-tool knowledge; web_search golden fixtures), plus typed enablement via the
  new two-variant `Tool` (`Custom` | `Provider{kind,name,config}` — hand-rolled
  serde keyed on the presence of the wire `type` key). BREAKING: `Tool` is now an
  enum (wire-compatible for custom tools). Additive `v=1` event kinds — no
  `EVENT_SCHEMA_VERSION` bump. No WebSearch/citation normalization; the
  `usage.server_tool_use.*` counter stays deferred (rides `provider_detail`).
  Non-Anthropic dialects fail fast: `Tool::Provider` → exit 64 at encode
  (openai/google/ollama/responses); `Content::ServerTool*` → exit 64
  (openai/ollama/responses) or dropped (google).
- **Automatic prompt caching (Anthropic)** — the Anthropic encoder now places
  `cache_control:{"type":"ephemeral"}` markers by itself, from the request's own
  shape: a head mark always (last `system` block, else last `tools` object, else
  none), a rolling mark on the last eligible block of the last non-assistant
  message when the request is an ongoing conversation (a lone trailing-assistant
  prefill never triggers), and one intermediate mark 20 eligible blocks behind
  the rolling mark on long transcripts. Never more than 3 marks; `thinking` /
  `redacted_thinking` blocks are skipped; TTL is never emitted (the renewing
  5-minute default). Every other dialect caches by prompt prefix on the provider
  side — nothing to declare, zero code. Cache effect stays observable through
  `Usage.cache_read_tokens`/`cache_write_tokens`; `--raw` bypasses the policy
  (e.g. for non-recurring replays that should not pay the cache-write premium).

### Removed

- **BREAKING — the `req.cache` breakpoint surface** (`cache` field,
  `CacheBreakpoint`/`CacheAnchor`/`CacheTtl` types, and their exit-64
  validations), which landed on `main` after 0.0.2 and never shipped in a
  release. Caching is brazen-owned policy with zero canonical surface — no
  field, no flag, no config key. A piped request still carrying the old
  `"cache": [...]` key is no longer understood: it falls into `extra`
  (fail-open) and rides to the wire, where the provider rejects the unknown
  key. No compat shim — pre-0.1 the type break is sanctioned.

### Fixed

- **Encoder param-fidelity sweep (bl-a9e2) — three defects.**
  - **Reasoning × sampling on the OpenAI dialects.** `openai_chat` and
    `openai_responses` emitted `temperature`/`top_p` even when `reasoning` was set,
    which 400s the exact models that accept `reasoning` (o-series/gpt-5) — the
    Anthropic encoder already dropped them, an asymmetry with no rationale. Both
    dialects now OMIT `temperature`/`top_p` when `reasoning` is set (the params stay
    on the canonical request for every other protocol). Additionally, `openai_chat`
    now emits `max_completion_tokens` instead of the deprecated `max_tokens` when
    `reasoning` is set — `req.reasoning` IS the explicit reasoning-model signal (no
    model-name sniffing), so a reasoning request riding a row whose `body_defaults`
    fills `max_tokens` no longer 400s. Responses is unaffected (always
    `max_output_tokens`). Specs: openai-chat-mapping.md §2.1/§2.7, providers.md §3.2.
  - **Responses silently dropped typed `parallel_tool_calls`.** The wire supports a
    top-level `parallel_tool_calls`, but the Responses encoder emitted nothing — a
    silent drop of a supported typed field. It now rides top-level exactly as Chat
    Completions. Specs: providers.md §3.2, §9 CR-R1; the genuine empty-set drops on
    Google/Ollama (neither wire has the knob) are now documented (providers.md §4.2/§5.3).
  - **Anthropic folded `disable_parallel_tool_use` onto every `tool_choice`.** With
    `parallel_tool_calls: false` the encoder added `disable_parallel_tool_use:true` to
    ALL non-empty tool_choice objects, including `{type:"none"}` and `{type:"tool"}`
    where the field is undocumented/nonsensical. The fold is now RESTRICTED to
    `auto`/`any`; with `none`/`tool` the `false` intent is inexpressible and drops.
    Spec: anthropic-messages.md §2.7.

- **Anthropic silently dropped mid-transcript `Role::System` messages.** The
  encoder did a bare `continue` on `Role::System` in its `messages[]` loop, so an
  in-band system turn a caller re-fed vanished from the wire — a silent content
  loss. It now **hoists** into the one top-level `system` array, matching
  architecture.md §3.1 ("Anthropic hoists either to its top-level `system`") and
  the already-correct Google adapter: `req.system` blocks first, then each
  `Role::System` message's blocks in transcript order. The slot stays text-only —
  a non-`Text` block in a hoisted `Role::System` message rejects with exit 64,
  the same rule `req.system` already followed. No dialect drops `Role::System`
  now: openai-chat / ollama / openai-responses pass it through in position (native
  system role), Google hoists like Anthropic.

- **Google `Image{Url}` implied support it did not have.** The encoder mapped a
  canonical `Image{Url{url}}` to `{fileData:{fileUri:url}}`, but Gemini's
  `fileData.fileUri` references only files uploaded to the Google Files API (or a
  Vertex `gs://` GCS URI) — it **cannot fetch an ordinary `https://…png`** and
  generally wants a `mimeType` sibling brazen cannot infer from a URL — so a
  normal web-URL image silently produced a request the provider rejected with a
  confusing 400. It now **rejects at encode** with `Error{ParseInput}` (exit 64),
  a message naming the limitation and the remedy (download and re-send as base64 →
  `inlineData`; brazen never adds the round-trip), matching architecture.md §3.1
  (a wire slot that cannot express the content rejects, never mistranslates) and
  the sibling Ollama base64-only-slot rule (providers.md §5.4 CR-O2). A total
  reject, not prefix-sniffing on Google-file/GCS URIs (the mimeType gap and the
  URL-namespace coupling sink the narrowing — providers.md §4.3, §9 CR-G3).

### Added

- **Request-time reasoning — `--reasoning low|medium|high`** — a portable effort
  knob mapped to each provider's native shape: OpenAI `reasoning.effort` /
  `reasoning_effort`, Anthropic extended thinking (`thinking.budget_tokens`, with
  `max_tokens` auto-raised to satisfy `max_tokens > budget_tokens` and
  `temperature`/`top_p` dropped as that API requires), Google
  `thinkingConfig.thinkingBudget`, and Ollama `think`. `--thinking` stays
  display-only — it *shows* reasoning, it does not *request* it. Provider-exact
  shapes remain available via a row's `body_defaults`, and `unsupported_body_keys
  = ["reasoning"]` opts a backend out.

- **Model cache learns from success** — a generation that names a model the cache
  cannot place and comes back `2xx` now appends that one model to the provider's
  cache. So a single `bz --provider X --model some-model "hi"` seeds the cache and
  the next bare `bz "…"` defaults to it — making zero-config "just work" even for a
  provider whose `--list-models` endpoint is broken or never run. It records only
  the model you chose and the provider accepted; it never lists behind your back.

## [0.0.1] — 2026-06-29

First published release. The core vertical slice — one canonical request and
`Event` stream, normalized across five provider protocols — is in and tested
end-to-end.

### Added

- **Protocols** — OpenAI `chat/completions`, OpenAI `responses` (ChatGPT/Codex),
  Anthropic `messages`, Google `generative-ai`, and Ollama (NDJSON), all
  normalized to one canonical request + `Event` stream. An executable
  single-source-of-truth test proves all five basic fixtures decode to the same
  `Vec<Event>`.
- **Providers** — OpenAI, Anthropic, Mistral, Google, and local Ollama, added as
  config rows. Mistral is the severability floor: one row, zero Rust (it reuses
  the OpenAI dialect verbatim).
- **Auth** — API key (`x-api-key` or `Authorization: Bearer`, chosen by row
  data), keyless (`none`, for local Ollama), and OAuth2 / SSO with silent
  refresh, including Sign in with ChatGPT via `bz --login`.
- **Routing** — a model owns its provider by an exact alias or a prefix family
  (`claude-`, `gpt-`, …), so `--provider` is droppable for an unambiguous model;
  ambiguity and missing/unknown providers surface as a clean config error.
- **Output** — streamed text (default), `--thinking`, `--json` (canonical NDJSON
  events), and `--raw` (lossless passthrough). A full sysexits-style exit table
  (0 / 64 / 66 / 69 / 70 / 77 / 78) and `BrokenPipe` → 141.
- **Config** — one schema folded flags > env > file > built-in defaults;
  `--dump-config` prints the merged config with secrets redacted.
- **Model discovery** — `bz --list-models` over a lazy live-probe cache.
- **Transport** — a blocking, rustls-backed `ureq` client (no OpenSSL, no async
  runtime) with config-driven connect / response / idle timeouts.

The pure library is held at 100% line coverage; the data plane is smoke-tested
live against Anthropic and OpenAI.

[Unreleased]: https://github.com/mudbungie/brazen/compare/v0.0.2...HEAD
[0.0.2]: https://github.com/mudbungie/brazen/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/mudbungie/brazen/releases/tag/v0.0.1
