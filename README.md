# brazen

**`brazen`** (binary **`bz`**) — a stateless, swiss-army-knife adapter for every LLM
provider and protocol. Pipe a request in, stream a normalized response out.

One small Rust binary that speaks OpenAI `chat/completions`, OpenAI `responses`,
Anthropic `messages`, and Google `generative-ai` across providers (OpenAI, Anthropic,
Mistral, Google, local Ollama, …), handling API-key and OAuth/SSO auth. It is a low-level
building block for agents.

> The *brazen head* was a brass automaton that answered any question put to it. Pipe in a
> question; it speaks the answer.

## Status

**Design phase.** We design first (specifications in [`specs/`](specs/)), implement second.
No implementation has landed yet. The roadmap is tracked in `bl` (balls).

## Principles

- **Stateless.** A pure `stdin → stdout` filter. The only disk it touches is XDG-standard
  config and credentials.
- **Single source of truth.** One canonical model; every protocol maps to and from it.
- **Deep, narrow interface.** Adding a provider / protocol / auth model is *data*, not a new
  core code path.
- **Strict POSIX.** Predictable streaming, exit codes, and signal handling.
- **100% test coverage**, enforced by the pre-commit hook. Code files capped at 300 lines.

## Layout

- [`specs/`](specs/) — design specifications (living documents). Start at
  [`specs/README.md`](specs/README.md).
- `Makefile` — build / test / coverage / lint targets (`make help`).
- `.githooks/pre-commit` — runs the full `make check` gate (fmt + clippy + 100% coverage)
  + the 300-line code-file cap, on commit and on `bl close`.

## Build (once implementation lands)

```sh
make hooks   # one-time per clone: enable the pre-commit gate
make check   # fmt + clippy + 100% coverage gate
```

## License

TBD.
