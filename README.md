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

**Early implementation.** We design first (specifications in [`specs/`](specs/)), implement
second. Landed so far: the canonical model (`CanonicalRequest`, the `Event` taxonomy) and the
error model (`CanonicalError`, `ExitClass`, the pure `retryable`/`exit_code` tables), plus the
pure pipeline — input resolution (`open_input`: stdin == `--input FILE`), canonical-in parsing
(`parse`), and the output projections (`NdjsonSink`/`TextSink`/`RawSink`) with the `pump` loop
(last-error-wins exit, `BrokenPipe` → 141) in the `brazen` lib, with the `bz` bin shim. The
**seams** are in place too: the `Protocol`, `Auth`, `Transport`, `CredStore`, and `Clock`
traits, the data records they exchange (`WireRequest`, `ProviderCtx`/`AuthCtx`, `Provider`,
`ProtocolId`/`AuthId`, `Cred`, `Secret`, `Frame`/`Framing`/`DecodeState`), the `Registry` that
dispatches by id without matching a vendor name, and the shared test doubles (`MockTransport`,
in-memory `CredStore`, `FakeClock`) under `brazen::testing`. The first concrete impls have
landed: the v0.1 data-plane auth — `ApiKeyAuth`/`BearerAuth` (secret resolution `inline_key →
store → MissingCreds/77`, the data-driven `x-api-key`/`Authorization: Bearer` header write, the
inline-key bypass that reads no store), registered in `Registry::builtin`; and the **shared
transport framers** — the `Decoder` trait + `Framing::decoder()`, with `SseDecoder` (blank-line
frames, `event:`/`data:` extraction, partial-frame & partial-UTF-8 buffering), the
`NdjsonDecoder` line-framer, and the lossless `IdentityDecoder` (`--raw`), verified deterministic
under adversarial rechunking (`OneByte`/`MidData`/`MidUtf8`/`MidJsonNumber`/`WholeFixture`). The
concrete protocol and transport impls (and `OAuth2`) remain spec-only. The roadmap is tracked in
`bl` (balls).

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

MIT — see [`LICENSE`](LICENSE).
