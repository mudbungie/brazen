# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is hand-authored: it is the deliberate, human-readable record of what
each release ships. `release-plz` prepends future versions above the entries
below — see the "Releasing" section of the README.

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/mudbungie/brazen/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/mudbungie/brazen/releases/tag/v0.0.1
