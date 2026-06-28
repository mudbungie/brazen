+++
title = "Expose the typed canonical I/O interface: a CanonicalRequest->Iterator<Event> entry point + re-expose the canonical vocabulary; rework interface-parity oracle to entry-point type-closure"
created = 1782678108
updated = 1782678119
claimant = "Snaring"
priority = 5
tags = ["interface-review", "design"]
+++
## Context (follow-up to bl-46e6)

bl-46e6 narrowed the public lib surface to the CLI-reachable set, using a parity oracle
of 'names the bz binary references'. That oracle UNDER-captured the interface: bz is a
thin byte-shim (stdin -> run() -> stdout) that pipes bytes and never NAMES CanonicalRequest
or Event, so the typed canonical I/O fell off the public surface. But the canonical model
(architecture.md S3, 'single source of truth') IS the contract; NDJSON/JSON are just its
serialization. Typed request-in / event-out are the same interface, one encoding removed.

## Decision (owner)

Expose the typed inputs and outputs: those ARE the interface. Do it so the parity invariant
stays MECHANICAL (no hand-maintained allowlist) — which requires a typed entry point, not a
bare re-pub (else both oracles flag the vocab as dead and force an allowlist).

## Deliverable

1. A typed entry point exposing the pure pipeline: CanonicalRequest -> Iterator<Event>
   (the canonical generate path minus byte-serialization). run() becomes the byte adapter
   over it (parse bytes -> typed core -> serialize via the sink; exit-code helper folds the
   event stream). --raw stays a byte passthrough (no typed events).
2. Re-expose the canonical I/O vocabulary: CanonicalRequest, Message, Content, Role, Tool,
   ToolChoice, ImageSource (input); Event, ContentKind, Delta, Usage, FinishReason,
   EVENT_SCHEMA_VERSION (output). (CanonicalError/ErrorKind/Model already public.)
3. Rework tests/interface_parity.rs: public surface == transitive TYPE-CLOSURE of the public
   entry-point signatures (mechanical, forward-compatible, no allowlist), replacing/augmenting
   the bin-reference oracle. The typed entry point's signature pulls the whole vocab into the
   closure automatically.
4. Living docs: update architecture.md S1 (the spine — factor out the typed core) and S9.8
   (the invariant + the corrected oracle; the byte-shim-names-it definition was the bug).

Type: DESIGN (spine refactor + the typed seam + reworked parity harness).