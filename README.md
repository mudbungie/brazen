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
landed: the v0.1 data-plane auth — `StaticSecretAuth` (secret resolution `inline_key →
store → MissingCreds/77`, the data-driven `x-api-key`/`Authorization: Bearer` header write, the
inline-key bypass that reads no store), one impl behind both the `api_key` and `bearer` ids in
`Registry::builtin`; and the **shared
transport framers** — the `Decoder` trait + `Framing::decoder()`, with `SseDecoder` (blank-line
frames, `event:`/`data:` extraction, partial-frame & partial-UTF-8 buffering), the
`NdjsonDecoder` line-framer, and the lossless `IdentityDecoder` (`--raw`), verified deterministic
under adversarial rechunking (`OneByte`/`MidData`/`MidUtf8`/`MidJsonNumber`/`WholeFixture`).
**Config resolution** has landed too: the one `PartialConfig` schema in four instances
(flags/env/file/embedded `defaults.toml`), the associative `flags.or(env).or(file).or(defaults)`
fold, the injected `EnvSnapshot` projection, `into_resolved` with model→provider routing as a
query over rows (ambiguity and missing/unknown/incomplete providers all surfaced as `Config`/78),
`fill_absent` (config fills only the gen fields the request omits), and `--dump-config`
(`dump_config`) with secrets elided to the inert `"<redacted>"` sentinel — all pure over injected
inputs. The first **protocol impl** has landed: `anthropic_messages` (`Protocol::encode`/`decode`
for `POST /v1/messages`) — system hoisting, `Role::Tool`→`tool_result`, `stop`→`stop_sequences`,
thinking/redacted-thinking with verbatim signature/data, text-only-slot rejection, `extra`
top-level merge; and a streaming `decode` state machine (content-block triples, cumulative
`Usage`, `stop_reason`→`FinishReason` incl. `pause_turn`/`refusal`, mid-stream/whole-body
`error`→`Error`), proven against golden `.sse` fixtures under adversarial rechunking. The second
protocol impl has landed too: `openai_chat` (`Protocol::encode`/`decode` for `POST
/chat/completions`) — nested function tools, `tool_choice` spellings (`Any`→`"required"`), content
string-or-array, base64→data-URI images, `Role::Tool` fan-out with textual `is_error`,
`stream_options.include_usage`, thinking-drop; and a streaming `decode` state machine over
positional `choices[0].delta` (synthesized `MessageStart`/`ContentStart`, `arguments`→`JsonDelta`,
`finish_reason`→`FinishReason`, the trailing usage chunk, `[DONE]`→terminated, 4xx/5xx body parse),
with an executable single-source-of-truth property test proving the OpenAI-basic and
Anthropic-basic fixtures decode to one canonical `Vec<Event>`. The transport impls and `OAuth2`
remain spec-only. The roadmap is tracked in `bl` (balls).

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
