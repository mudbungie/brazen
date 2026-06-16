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
Anthropic-basic fixtures decode to one canonical `Vec<Event>`. **Three more protocol impls have
landed** (providers spec): `openai_responses` (`POST /responses` — `system`→`instructions`, typed
`input[]`, `max_tokens`→`max_output_tokens`, FLAT tools; a `response.*` SSE state machine keyed off
the wire `output_index`, `response.completed`→terminated), `google_generative_ai` (model in the URL
path, `user`/`model` roles, structured `inlineData` images, the `x-goog-api-key` auth header as
pure **row data** read by the shared `ApiKeyAuth` — no new `Auth` impl, the last chunk's non-null
`finishReason` the terminator), and `ollama_chat` (**NDJSON** framing as one DATA return, params
nested under `options`, whole tool calls, `{"done":true}` the terminator) — Google/Ollama
synthesizing block structure with a monotonic, never-stored `open.len()` index and closing at the
terminal drain, all with golden fixtures under adversarial rechunking. **Mistral** is the
severability floor: **one `[[provider]]` row, zero Rust**, reusing `openai_chat`+`bearer` verbatim.
The cross-provider property test now proves **all five** basic fixtures reduce to one canonical
`Vec<Event>`. The **`run` spine** now assembles
the whole vertical slice: argv flag parsing (`cli::parse_args` → `Flags`), `read_request`
(positional-prompt XOR stdin canonical request; both → exit 64), the config-file read, and the
`resolve → dispatch → encode → auth → send → frame → decode → project` pipeline, with the full
exit-code table (0/64/66/69/70/77/78 and `BrokenPipe`→141) — driven end-to-end against
`MockTransport` at 100% line coverage. `os::browser_argv` (the one OS-`match`, tested as data for
all three targets) and the `bz` `main` shim (SIGPIPE restore + native impl wiring) round it out.
The **real network `HttpTransport`** has landed behind the `Transport` seam in the `bz` bin crate
(`bz/src/transport.rs`): a blocking, rustls-backed `ureq` round-trip (rustls + bundled
`webpki-roots`, no OpenSSL, no async runtime), with the non-2xx status peeked onto
`TransportResponse.status` and `into_reader()` streamed chunk-by-chunk as
`Iterator<io::Result<Bytes>>`; connect/DNS/TLS/timeout failures map to a `Transport` error (exit
69). `ureq` is a dependency of the **`bz` crate only** — the `brazen` lib's dependency graph has no
`ureq`/`libc`, so the pure core *cannot* link the network client (the network-free invariant is the
crate graph's, not discipline's). Smoke-tested live against Anthropic and OpenAI.
The **OAuth2 capability** has now landed too: the five pure builders/parsers (`build_authorize_url`
PKCE-S256, `parse_callback` CSRF, the one `build_token_exchange_request` over a three-armed `Grant`,
`parse_token_response` with an absolute `expires_at`, `is_expired`), `OAuth2::apply`'s silent
in-band refresh through the same `Transport` seam (persist-then-use, the `anthropic-beta`
auth-mode-dependent header, not-logged-in/refresh-failed → 77), and the quarantined `bz login`
control plane — Device flow (RFC 8628) and AuthCode + loopback (RFC 8252) behind injected
`BrowserLauncher`/`CodeReceiver`/`Pacer` seams, fully offline-tested via fakes + `MockTransport`/
`ScriptedTransport` + `FakeClock`. The native `SystemBrowserLauncher`/`LoopbackReceiver`/atomic
0600 `XdgCredStore`/OS-RNG live in the coverage-excluded `bz` shim. The roadmap is tracked in `bl`
(balls).

## Principles

- **Stateless.** A pure `stdin → stdout` filter. The only disk it touches is XDG-standard
  config and credentials.
- **Single source of truth.** One canonical model; every protocol maps to and from it.
- **Deep, narrow interface.** Adding a provider / protocol / auth model is *data*, not a new
  core code path.
- **Strict POSIX.** Predictable streaming, exit codes, and signal handling.
- **100% test coverage**, enforced by the pre-commit hook. Code files capped at 300 lines.

## Layout

Two workspace crates: the **`brazen` lib** (root package — the pure, network-free core; pure-Rust
deps only) and the **`bz` bin crate** ([`bz/`](bz/) — the impure native shim that owns the only
`ureq`/`libc` usage). `bz` depends on `brazen`, never the reverse, so the lib cannot link the
network client.

- [`specs/`](specs/) — design specifications (living documents). Start at
  [`specs/README.md`](specs/README.md).
- `Makefile` — build / test / coverage / lint targets (`make help`).
- `.githooks/pre-commit` — runs the full `make check` gate (fmt + clippy + 100% coverage)
  + the 300-line code-file cap, on commit and on `bl close`.
- [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — the `make check` gate (run once,
  it is platform-independent) plus the portability matrix.
- [`.github/workflows/release.yml`](.github/workflows/release.yml) — tag-triggered publish.

## Build

```sh
make hooks   # one-time per clone: enable the pre-commit gate
make check   # fmt + clippy + 100% coverage gate
```

## Platform support

CI builds **and tests** the workspace on every target on a native runner — no
cross-emulation, so portability is proven by execution:

| OS | x86_64 | aarch64 | static |
|---|---|---|---|
| Linux | `x86_64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu` | `x86_64-unknown-linux-musl` |
| macOS | `x86_64-apple-darwin` | `aarch64-apple-darwin` | — |
| Windows | `x86_64-pc-windows-msvc` | `aarch64-pc-windows-msvc` | — |

The matrix stays green because the native surface is deliberately tiny: **no build
scripts, no C dependencies, no codegen** — pure `cargo build`. TLS is `rustls`
(pure-Rust, no OpenSSL/`pkg-config`), there is no async runtime, and the `brazen`
lib has **zero platform-specific code** — the one OS branch (browser-launch argv)
lives behind the `BrowserLauncher` seam in the `bz` shim and is tested as data for
all three OSes. The single conditional dependency (`libc`, for restoring the Unix
SIGPIPE disposition) is `bz`-only and `[target.'cfg(unix)']`-gated.

### Windows secret-at-rest: documented limitation

Stored credentials are one JSON file per provider, written atomically (temp-file +
rename). On **Unix** the file is forced to mode **`0600`** at write time. On
**Windows** the file simply **inherits the user-profile directory ACL** — there is
**no DPAPI encryption and no explicit ACL hardening**. This is a deliberate v0.1
trade-off, *not* a code branch: adding DPAPI would pull in a Windows-specific C
dependency and break the no-C-deps, single-binary portability story above. Treat
secrets on a shared or improperly-permissioned Windows profile as readable by other
accounts on that machine. (See architecture spec §6.4 / §10.)

## Releasing (publishing to crates.io)

> **Pre-release: publishing is guarded off.** The version is `0.0.0` and both
> crates carry `publish = false`. The metadata and workflow below are fully wired
> but inert — going live is one deliberate switch: bump the version and drop the
> `publish = false` guard on both crates.

Both crates publish to crates.io: the **`brazen`** library and the **`bz`** binary
(`cargo install bz`). Shared metadata (version, license, repository, keywords)
lives once in `[workspace.package]`; each crate inherits it. Because `bz` depends
on `brazen`, the **lib publishes first**:

```sh
cargo publish -p brazen   # lib first
cargo publish -p bz       # then the bin (resolves brazen from the registry)
```

Pushing a `v*` tag runs this automatically via `release.yml` (gated by `make check`,
using the `CARGO_REGISTRY_TOKEN` secret) — a deliberate step, like pushing to origin.

## License

MIT — see [`LICENSE`](LICENSE).
