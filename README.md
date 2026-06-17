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
`Registry::builtin` — plus `NoAuth` behind the `none` id for keyless providers (local Ollama: no
cred read, no header written); and the **shared
transport framers** — the `Decoder` trait + `Framing::decoder()`, with `SseDecoder` (blank-line
frames, `event:`/`data:` extraction, partial-frame & partial-UTF-8 buffering), the
`NdjsonDecoder` line-framer, and the lossless `IdentityDecoder` (`--raw`), verified deterministic
under adversarial rechunking (`OneByte`/`MidData`/`MidUtf8`/`MidJsonNumber`/`WholeFixture`).
**Config resolution** has landed too: the one `PartialConfig` schema in four instances
(flags/env/file/embedded `defaults.toml`), the associative `flags.or(env).or(file).or(defaults)`
fold, the injected `EnvSnapshot` projection, `into_resolved` with model→provider routing as a
query over rows — a row OWNS a model by an exact `model_aliases` entry or a `model_prefixes`
family claim (`["claude-"]`, `["gpt-", …]`), so `bz -m claude-… "q"` routes with no `--provider`
(ambiguity and missing/unknown/incomplete providers all surfaced as `Config`/78),
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
crate graph's, not discipline's). Its timeouts are **config**, not magic constants: `timeout_connect`
/ `timeout_response` / `timeout_idle` fold through the normal config layer (flags/env/file, floored by
`data/defaults.toml`), and `run` stamps them onto the `WireRequest` per request. `timeout_idle` is an
inter-chunk bound on the streaming body — reset on every chunk — so a provider that sends headers then
stalls can no longer hang `bz` forever, yet a long-but-live generation is never truncated. Smoke-tested
live against Anthropic and OpenAI; the four
later providers (`openai_responses`, `google_generative_ai`, `ollama_chat`, and the zero-Rust
`mistral` row) had their hand-authored golden fixtures validated shape-by-shape against the
authoritative provider specs — Google's free-form `FunctionResponse` Struct (`{"result": …}` is
an officially-named acceptable key), Ollama's `api.md` (`options.num_predict`, bare-base64
`images`, object-valued tool `arguments`, the `done`/`done_reason`/`prompt_eval_count`/`eval_count`
terminator), OpenAI's Responses OpenAPI schema (`output_index`/`content_index`, the function-call
item's `call_id`, `usage.input_tokens_details.cached_tokens`), and Mistral's reference (`"required"`
≡ `"any"`, `max_tokens` honored) — confirming the OpenAI-chat dialect is reused verbatim. `make
smoke` (`scripts/smoke.sh`) re-runs tiny live requests per provider on demand — a happy probe on each
input channel (the positional prompt and a canonical request piped on stdin), both output-mode
contracts (`--json`, asserting a `MessageStart`(v=1)…`End` NDJSON envelope; `--raw`, asserting
verbatim provider bytes carry none of brazen's framing), and a bad-key error probe — skipping any
row whose key env-var is unset. It requests no `--stream`: streaming is brazen's implicit default, so
the probes catch a regression that silently stops requesting it; `BZ_SMOKE_<PROVIDER>_MODEL` repoints
a row at a model the box actually has (e.g. a pulled `ollama` tag). The OAuth2/SSO data plane is
covered too (see the **Live conformance suite** below).
The **OAuth2 capability** has now landed too: the five pure builders/parsers (`build_authorize_url`
PKCE-S256, `parse_callback` CSRF, the one `build_token_exchange_request` over a three-armed `Grant`,
`parse_token_response` with an absolute `expires_at`, `is_expired`), `OAuth2::apply`'s silent
in-band refresh through the same `Transport` seam (persist-then-use, the `anthropic-beta`
auth-mode-dependent header, not-logged-in/refresh-failed → 77), and the quarantined `bz login`
control plane — Device flow (RFC 8628) and AuthCode + loopback (RFC 8252) behind injected
`BrowserLauncher`/`CodeReceiver`/`Pacer` seams, fully offline-tested via fakes + `MockTransport`/
`ScriptedTransport` + `FakeClock`. The native `SystemBrowserLauncher`/`LoopbackReceiver`/atomic
0600 `XdgCredStore`/OS-RNG live in the coverage-excluded `bz` shim. The roadmap is tracked in `bl`
(balls). The OAuth row also carries the auth §10 additions as data — a configurable loopback
`redirect` (host/port/path, defaulting to today's ephemeral `127.0.0.1/callback`), extra
`authorize_params`, an `account_header` whose value is the credential's `account_id`, and a token
`exp` read from the access-token JWT when the endpoint returns no `expires_in` — so a provider like
OpenAI's "Sign in with ChatGPT" is a config row with **no new vendor branch** in the core.

## Sign in with ChatGPT (OpenAI SSO)

`bz` can authenticate against a ChatGPT subscription using the same OAuth flow the Codex CLI uses.
There is no built-in OpenAI OAuth row (the core ships no vendor login policy — auth §7); paste this
row into your `config.toml` (`$XDG_CONFIG_HOME/brazen/config.toml` or `$BRAZEN_CONFIG`), then run
`bz login openai-chatgpt --browser`:

```toml
[[provider]]
name       = "openai-chatgpt"
base_url   = "https://chatgpt.com/backend-api/codex"
protocol   = "openai_responses"
auth       = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }

[provider.oauth]
authorize_url    = "https://auth.openai.com/oauth/authorize"
token_url        = "https://auth.openai.com/oauth/token"
client_id        = "app_EMoamEEZ73f0CkXaXp7hrann"
scope            = "openid profile email offline_access api.connectors.read api.connectors.invoke"
redirect         = { host = "localhost", port = 1455, path = "/auth/callback" }
authorize_params = [["id_token_add_organizations", "true"], ["codex_cli_simplified_flow", "true"], ["originator", "codex_cli_rs"]]
account_header   = "ChatGPT-Account-ID"
beta_headers     = [["originator", "codex_cli_rs"]]

[provider.body_defaults]   # request-body fields this backend always needs
store  = false             # the Codex backend 400s unless store:false

# Canonical request-body fields this backend REJECTS — the inverse of body_defaults.
# brazen strips each before encoding, so a stray --temperature/--top-p/--max-tokens
# never reaches the wire (the Codex backend 400s on all three; see specs/config.md §4.1).
unsupported_body_keys = ["max_tokens", "temperature", "top_p"]
```

`[provider.body_defaults]` pins request-body fields a backend always requires so you don't
hand-craft them every call: `store = false` here makes
`bz --provider openai-chatgpt --model gpt-5.4 --system "…" "hi"` just work. (The Codex backend
also 400s unless `stream:true`, but that needs no pin — brazen always wire-streams, forcing
`stream:true` for every row, so the mandate is satisfied automatically; see `specs/config.md` §4.2.)
A `body_defaults`
value is a per-row default at the lowest precedence — an explicit flag or request field beats it.
A row that *requires* a token cap (standard providers) sets `body_defaults = { max_tokens = … }`;
the Codex row deliberately pins none (its backend rejects `max_output_tokens`). See
[`specs/config.md` §4.1](specs/config.md).

`unsupported_body_keys` is the **inverse** of `body_defaults`: where `body_defaults` *fills* a
field the backend always needs, `unsupported_body_keys` *strips* a field the backend cannot accept.
The Codex backend 400s on `temperature`, `top_p`, and `max_output_tokens` with
`{"detail":"Unsupported parameter: …"}` (validated live 2026-06-17) — `bz` renames
`max_tokens`→`max_output_tokens` per the Responses spec, but this one backend forbids the standard
sampling/length params. With the three keys listed above, `bz` silently drops them before encoding
(naming **canonical** fields — `max_tokens`, not the wire `max_output_tokens` — so the rename stays
owned by the encoder), so passing `--max-tokens`/`--temperature`/`--top-p` (or the same keys in the
input JSON) against this row no longer 400s — the value is normalized away, the request streams
normally. brazen now supplies or normalizes every one of this backend's quirks (`instructions`,
`store:false`, `stream:true`, and the three rejected params); none is left for the operator to honor
by hand.

The flow, the verified Codex wire facts behind each field, and the empirical risks still to confirm
end-to-end (e.g. the data-plane request shape against the `codex` backend) are documented in
[`specs/auth.md` §10](specs/auth.md).

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
make smoke   # live request per provider (real keys; skips providers whose key is unset)
```

## Live conformance suite

`make smoke` (`scripts/smoke.sh`) asks shallow questions — *did each provider with a key
return exit 0 + non-empty output on a good key (keeping `--json`/`--raw` output-mode shape),
and a correct non-zero exit + a non-empty surfaced provider error on a bad one?* It also probes the
**OAuth2 / SSO data plane** (bl-61a6): the real `AuthId::OAuth2` path via a stored `bz login
openai-chatgpt` cred, and the anthropic Max OAuth token (`sk-ant-oat01…`) through a bearer +
`anthropic-beta` oauth `--config` override — the token taken from `$ANTHROPIC_OAUTH_TOKEN`, else a
Claude Code login (`~/.claude/.credentials.json`) when `jq` is present; each SSO row SKIPs when its
credential is absent. The **live conformance suite**
(`bz/tests/live_conformance.rs`) asks the real one: *does one canonical request
produce the same NORMALIZED event grammar across every provider this box can
authenticate to?* That is the whole point of brazen, so this is the test that
proves it end-to-end against live endpoints.

It is **opt-in** and never part of `make check`/CI: it is `#[ignore]`d (run only
under `--ignored`) **and** env-gated on `BRAZEN_LIVE`, and the whole `bz` crate is
excluded from the coverage gate — so it adds no coverage obligation. Run it:

```sh
BRAZEN_LIVE=1 \
  BRAZEN_LIVE_OLLAMA_MODEL=llama3.2 \   # point each row at a model this box has
  OPENAI_API_KEY=sk-… \                 # any provider key you want exercised
  cargo test -p bz --test live_conformance -- --ignored --nocapture
```

**Providers are discovered at runtime.** For each row the harness looks for a
usable credential — a reachable keyless endpoint (Ollama), a stored `Cred` from
`bz login <provider>` (e.g. OpenAI "Sign in with ChatGPT"), or one of the row's
API-key env vars — and **skips, never fails,** any provider with none, printing
the reason (no silent truncation). A box with zero credentials is a clean no-op.

**Per authed provider it asserts the canonical surface:** the streamed-text event
grammar over `--json` (`message_start` → text `content_start` → `text_delta` →
`usage` → `finish` → terminal `end`), the `--text` projection, system/instructions
(every request carries a non-empty `system`), a tool round-trip where the row
supports it (a `tool_use` `content_start` + streamed `json_delta` arguments), and
error mapping (a deliberately bad model → exit 69). The `--raw` projection is
currently skipped pending **bl-080b** (the data-plane raw path sends an empty URL).

**Adding a provider is one `Row`** in the `TABLE` (no code branches — quirks are
DATA): set `provider`/`model`/`model_env`, the `auth` discovery strategy
(`Keyless { probe }` or `Keyed { env }`), and the per-row knobs (`max_tokens:
None` to omit it, `store_false`, `tools`). The harness drives the same assertions
for every row. (The codex backend's quirks — no `max_output_tokens`, explicit
`store:false`, required `instructions` — live entirely in its row as data,
validated live 2026-06-16.)

### OpenAI ChatGPT-SSO fuzz

Where the conformance suite drives the *one* happy path, `bz/tests/live_fuzz_openai.rs`
(**bl-b72f**) drives a *wide range of request shapes* at the live `openai-chatgpt`
codex backend — surfacing where brazen mis-encodes or mis-maps errors. It reuses the
conformance harness leaves (`live_support/exec.rs`, `…/grammar.rs`) verbatim, so it is
the same black-box, `#[ignore]`d, `BRAZEN_LIVE`-gated, coverage-excluded shape, and
skips (printed reason) without a `bz login openai-chatgpt` cred. Two families:

- **Error-conformance matrix** — the fully-valid codex body *minus one required field*
  (no `instructions` / no `store` / `stream:false`) and the unsupported `gpt-5-codex`
  model. Each must 400 → exit 69 **and** surface the service's own message — `"Instructions
  are required"`, `"Store must be set to false"`, `"Stream must be set to true"`, `"…not
  supported…"` — asserted end-to-end (the codex `{"detail":…}` body reaching the
  `CanonicalError` is what **bl-5fe6** fixed; an empty message here is a regression). These
  400 before generation, so they are ~free.
- **Request-shape acceptance** — well-formed variations (unicode/emoji content,
  multi-turn role ordering, a tool round-trip) that must return exit 0 + the canonical
  grammar. These GENERATE, so they are behind a SECOND opt-in, `BRAZEN_LIVE_FUZZ_SPEND=1`,
  and the run prints what ran vs was capped.

```sh
BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
  cargo test -p bz --test live_fuzz_openai -- --ignored --nocapture
```

(Raw-SSE golden capture for offline-decoder replay is intentionally *not* duplicated
here: the offline `response.*` decoder is already exhaustively fixture-tested in
`tests/responses_fixtures.rs` / `tests/responses_decode_errors.rs`, so this suite is
the request/error conformance the offline path structurally cannot reach.)

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
