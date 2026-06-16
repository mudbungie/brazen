# brazen specifications

Design-first: each capability is specified here **before** it is implemented. Specs are living
documents — edited like code, not frozen after writing. Git history is the changelog.

| Spec | Derives from |
|------|--------------|
| [Architecture & I/O Contract](architecture.md) | — |
| [OpenAI chat/completions mapping](openai-chat-mapping.md) | architecture |
| [Anthropic messages mapping](anthropic-messages.md) | architecture |
| [Auth, OAuth/SSO & credential store](auth.md) | architecture |
| [Config schema, resolution & compiled config](config.md) | architecture |
| [SSE / NDJSON decoder & DecodeState](sse-decoder.md) | architecture |
| [Provider rows: Mistral, responses, Google, Ollama](providers.md) | architecture |

## Conventions

- Files are descriptively named; there is no numbering (git history orders and records them).
- A spec is the deliverable of a `bl` design task: the task records the work, the file is the artifact.
- The mapping/auth/config/decoder/provider specs derive from `architecture.md` and must not
  contradict it — if one needs to, `architecture.md` changes first.
