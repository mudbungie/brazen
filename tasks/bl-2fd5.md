+++
title = "Event vocabulary forward-compat: #[non_exhaustive] + unknown-tolerant decode on Event/ContentKind/Delta; define the v=1 additive contract; reconcile architecture.md §3.2"
created = 1782588028
updated = 1782590278
claimant = "Ganglia"
priority = 5
tags = ["interface-review", "impl"]
+++
## Context

One-way-door review (2026-06-27). architecture.md §3.2 claims, about deferred server-tool
kinds: "adding a kind later is the empty-set rule run forward, **not a breaking change**."
Verified against `src/canonical/event.rs` — this is FALSE on both surfaces:

- `Event` is `#[serde(tag="type")]` with no `#[serde(other)]` and no `#[non_exhaustive]`;
  deserializing an unknown `"type"` ERRORS.
- `ContentKind` and `Delta` (externally tagged) likewise ERROR on an unknown variant.
- Only `FinishReason` got forward tolerance — a hand-rolled `Other(String)` catch-all
  (event.rs:111 + the `_ =>` arm at :158). So forward-compat is implemented for exactly
  one of four event sub-vocabularies.

Consequence: when web-search lands and we add `ContentKind::WebSearchResult`, we break
(a) every Rust consumer (no `#[non_exhaustive]`) and (b) every consumer parsing with these
serde types (no catch-all) — while the spec promises we won't. `MessageStart.v` is a
version NUMBER with no defined CONTRACT (Decision 9 says a bump = "backward-incompatible
Event change" but never says what is additive WITHIN a `v`).

## Decision (owner): agreed — fix it.

## Deliverable

1. **Define the `v=1` contract** (architecture.md §3.2 / Decision 9): within a fixed `v`,
   consumers MUST ignore unknown `type`/`kind`/`delta` values and unknown object fields;
   `v` bumps ONLY for removal / rename / semantic change. A new kind/event is additive.
2. **Make the types honor it**: give `Event`/`ContentKind`/`Delta` the same unknown-
   tolerant decode `FinishReason::Other` already has (an `Other`/skip path), and add
   `#[non_exhaustive]` to the public event enums (`Event`, `ContentKind`, `Delta`,
   `FinishReason`, `ErrorKind`, `Content`, `Role`). This dissolves the FinishReason-only
   asymmetry (our own "dissolve special cases" rule — `Other` is the general path, the
   others are the missing empty-set).
3. **Reconcile the §3.2 text** so doc and code agree (it currently lies).
4. **Rename the `Usage` fields to be token-explicit** (owner-decided 2026-06-27):
   `input`→`input_tokens`, `output`→`output_tokens`, `cache_read`→`cache_read_tokens`,
   `cache_write`→`cache_write_tokens` (event.rs:90-95). These are token counts
   (architecture.md §3.2 "token accounting"; they map from Anthropic `input_tokens`/
   `output_tokens`/`cache_read_input_tokens`/`cache_creation_input_tokens` and OpenAI
   `prompt_tokens`/`completion_tokens`), so the generic names read ambiguously and must be
   made specific BEFORE `v=1` freezes the wire vocabulary. This changes BOTH the public
   struct field names AND the `{"type":"usage",…}` NDJSON line — update the §5.2 sample,
   the per-provider decode mappings, and the golden fixtures. Also add a one-line contract
   note that the error event (`CanonicalError`/`ErrorKind`) is frozen — `v` is absent on an
   error-first stream, so the error schema has no version gate.

Type: **IMPLEMENTATION** (concrete type changes + a small contract definition + doc
reconcile). 100% coverage: the new unknown-tolerant arms need a deliberately-unknown
fixture, exactly like the existing `FinishReason::Other` bogus fixture (arch §9.5).