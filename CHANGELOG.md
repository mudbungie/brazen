# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is hand-authored: it is the deliberate, human-readable record of what
each release ships. `release-plz` prepends future versions above the entries
below — see the "Releasing" section of the README.

## [Unreleased]

## [0.0.3](https://github.com/mudbungie/brazen/compare/v0.0.2...v0.0.3) - 2026-06-30

### Other

- Implement first-class prompt caching: typed req.cache breakpoints -> Anthropic cache_control markers [bl-d25b]

### Added

- **First-class prompt caching — `req.cache` breakpoints → Anthropic
  `cache_control`** — a request-only, typed set of cache breakpoints (each an
  `anchor` of `tools` / `system` / `message{index}` plus a `ttl` of `5m` or `1h`)
  that the Anthropic encoder projects to a per-block `cache_control:{"type":
  "ephemeral"[,"ttl":"1h"]}` marker on the last wire block of the anchored region.
  Only Anthropic emits a marker; every other dialect caches by prompt prefix and
  ignores `cache` with no code. At most 4 breakpoints, each must resolve to a wire
  block (else exit 64) — validated Anthropic-encode-local. The `5m` default is
  emitted by omitting `ttl`; order is preserved.

## [0.0.2] — 2026-06-29

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

## [0.0.1] — 2026-06-23

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
