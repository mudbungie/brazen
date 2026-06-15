# brazen specifications

Design-first: each capability is specified here **before** it is implemented. Specs are
living documents — edited like code, not frozen after writing.

| Spec | Title | Status |
|------|-------|--------|
| [0001](0001-architecture.md) | Architecture & I/O Contract | accepted |
| 0002 | Canonical ⇄ OpenAI chat/completions mapping | planned |
| [0003](0003-anthropic-messages.md) | Canonical ⇄ Anthropic messages mapping | accepted |
| 0004 | Auth, OAuth/SSO & the credential store | planned |
| 0005 | Config schema, resolution & compiled config | planned |
| 0006 | SSE / NDJSON decoder & DecodeState | planned |
| 0007 | Provider rows: Mistral, responses, Google, Ollama | planned |

## Conventions

- Specs are numbered `NNNN-short-title.md`.
- A spec is the deliverable of a `bl` design task: the task records the work, the file is the
  artifact.
- Implementation specs (per-protocol and per-provider mappings) derive from spec `0001`; they
  must not contradict it — if they need to, `0001` changes first.
