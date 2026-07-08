# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is hand-authored: it is the deliberate, human-readable record of what
each release ships. `release-plz` prepends future versions above the entries
below — see the "Releasing" section of the README.

## [Unreleased]

### Added

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
