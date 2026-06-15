# brazen specifications

Design-first: each capability is specified here **before** it is implemented. Specs are
living documents — edited like code, not frozen after writing.

| Spec | Title | Status |
|------|-------|--------|
| [0001](0001-architecture.md) | Architecture & I/O Contract | drafting |

## Conventions

- Specs are numbered `NNNN-short-title.md`.
- A spec is the deliverable of a `bl` design task: the task records the work, the file is the
  artifact.
- Implementation specs (per-protocol and per-provider mappings) derive from spec `0001`; they
  must not contradict it — if they need to, `0001` changes first.
