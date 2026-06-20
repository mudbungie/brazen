# brazen

**`brazen`** (the **`bz`** command) — a stateless, swiss-army-knife adapter for every LLM
provider and protocol. Pipe a request in, stream a normalized response out.

One small Rust binary that speaks OpenAI `chat/completions`, OpenAI `responses`,
Anthropic `messages`, and Google `generative-ai` across providers (OpenAI, Anthropic,
Mistral, Google, local Ollama, …), handling API-key and OAuth/SSO auth. It is a low-level
building block for agents.

> The *brazen head* was a brass automaton that answered any question put to it. Pipe in a
> question; it speaks the answer.

## Install

```sh
cargo install brazen            # builds and installs the `bz` command
```

Or download a prebuilt `bz` for your platform from the [latest release][releases] — no Rust
toolchain required. Building from source needs Rust 1.85+.

[releases]: https://github.com/mudbungie/brazen/releases/latest

## Quickstart

```sh
# one-shot: key on the env, model picked by --model (which prefix-routes to its provider)
ANTHROPIC_API_KEY=sk-ant-... bz --model claude-sonnet-4-6 "What is the capital of France?"
```

Set a default model once and the prompt is all you need — the brazen head speaks the answer:

```sh
export ANTHROPIC_API_KEY=sk-ant-...     # or BRAZEN_API_KEY; or `bz login` for OAuth/SSO
export BRAZEN_MODEL=claude-sonnet-4-6
bz "What is the capital of France?"
bz "Summarize this: $(cat notes.txt)"     # feed data via the prompt (a positional prompt
                                          # overrides stdin; pipe a canonical JSON request with no arg)
```

More verbs:

```sh
bz login openai-chatgpt --browser        # OAuth / Sign in with ChatGPT — no API key
bz --provider openai --model gpt-5 "explain monads in one line"
bz list-models --provider anthropic      # discover the model ids a provider serves
bz --json "..."                          # canonical NDJSON event stream instead of text
```

## What works today

**Early implementation** — we design first (specifications in [`specs/`](specs/)), implement
second — but the core vertical slice is in and tested end-to-end:

- **Protocols** — OpenAI `chat/completions`, OpenAI `responses` (ChatGPT/Codex), Anthropic
  `messages`, Google `generative-ai`, and Ollama (NDJSON), all normalized to one canonical
  request + `Event` stream. An executable single-source-of-truth test proves all five basic
  fixtures decode to the *same* `Vec<Event>`.
- **Providers** — OpenAI, Anthropic, Mistral, Google, and local Ollama, added as config
  rows. Mistral is the severability floor: **one row, zero Rust** (it reuses the OpenAI
  dialect verbatim).
- **Auth** — API key (`x-api-key` or `Authorization: Bearer`, chosen by row data), keyless
  (`none`, for local Ollama), and OAuth2 / SSO with silent refresh, including **Sign in with
  ChatGPT** via `bz login`.
- **Routing** — a model owns its provider by an exact alias or a prefix family (`claude-`,
  `gpt-`, …), so `--provider` is droppable for an unambiguous model; ambiguity and
  missing/unknown providers surface as a clean config error.
- **Output** — streamed text (default), `--thinking`, `--json` (canonical NDJSON events), and
  `--raw` (lossless passthrough). A full sysexits-style exit table (0 / 64 / 66 / 69 / 70 /
  77 / 78) and `BrokenPipe` -> 141.
- **Config** — one schema folded **flags > env > file > built-in defaults**; `--dump-config`
  prints the merged config with secrets redacted.
- **Model discovery** — `bz list-models` over a lazy live-probe cache.
- **Transport** — a blocking, rustls-backed `ureq` client (no OpenSSL, no async runtime) with
  config-driven connect / response / idle timeouts.

The pure library is held at **100% line coverage**; the data plane is smoke-tested live against
Anthropic and OpenAI. The full design lives in [`specs/architecture.md`](specs/architecture.md).

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
also 400s unless `stream:true`, but that needs no pin — brazen's stream-native global default is
`true`, so the mandate is satisfied by default; a row that wanted to FORCE it could still pin
`body_defaults = { stream = true }`, and `--no-stream` against this backend honestly surfaces the
provider's 400 rather than silently reverting — see `specs/config.md` §4.2.)
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

## OAuth providers in general (and a note on Anthropic)

The OAuth machinery is **vendor-blind** and reachable by config: a provider row with
`auth = "oauth2"` resolves like any other, given a `[provider.oauth]` block of operator-supplied
values. Nothing about any specific vendor is built in — brazen ships **no** OAuth row and bakes in
**no** vendor login policy ([`specs/auth.md` §7](specs/auth.md),
[architecture.md §13](specs/architecture.md) item 3). The fields the row understands, all data:

```toml
[[provider]]
name       = "my-oauth"        # an ALTERNATE row; claims no model_prefixes ⇒ reach it via --provider
base_url   = "https://…"
protocol   = "anthropic_messages"   # or openai_responses / openai_chat / …
auth       = "oauth2"          # the OAuth2 impl: silent in-band refresh
api_header = { name = "Authorization", scheme = "bearer" }

[provider.oauth]
authorize_url   = "https://…/authorize"   # operator-supplied; nothing vendor-specific is compiled in
token_url       = "https://…/token"       # token exchange AND silent refresh
client_id       = "…"
scope           = "…"
beta_headers    = [["…", "…"]]            # auth-mode-DEPENDENT headers, sent ONLY under OAuth (auth §4)
system_preamble = "…"                     # text the request's system must LEAD with, prepended in resolution (auth §4.1)
```

A row may also carry an `ambient` block to discover a credential another tool already wrote
(see **Ambient credential discovery** earlier in this README), and `bz login <provider> --browser`
runs the loopback flow when the vendor's registered redirect is a loopback URL. See
[`specs/auth.md`](specs/auth.md) §4–§7 for the full mechanism.

**Anthropic, specifically.** A Claude **subscription** OAuth token (an `sk-ant-oat01…` rather than
an `sk-ant-api…` key) is intended for Anthropic's own tools; Anthropic's terms restrict third-party
use of it. brazen does not configure that path for you, and we don't ship a recipe for it — the
generic `oauth2` mechanism above exists, but supplying the endpoints, client id, scope, and any
required system lead is **your** decision and **your** responsibility under those terms. A normal
`sk-ant-api…` key needs none of this; it just works through the built-in `anthropic` row.

## Principles

- **Stateless.** A pure `stdin → stdout` filter. The only disk it touches is XDG-standard
  config and credentials.
- **Single source of truth.** One canonical model; every protocol maps to and from it.
- **Deep, narrow interface.** Adding a provider / protocol / auth model is *data*, not a new
  core code path.
- **Strict POSIX.** Predictable streaming, exit codes, and signal handling.
- **100% test coverage**, enforced by the pre-commit hook. Code files capped at 300 lines.

## Layout

One crate, **`brazen`** — `cargo install brazen` builds the **`bz`** command (the `balls`->`bl`
pattern). The pure, network-free core is the library (`src/lib.rs`); the impure native shim — the
only `ureq`/`libc` user — is the `bz` bin (`src/main.rs`) and [`src/native/`](src/native/). Now that
it is one crate, [`tests/purity.rs`](tests/purity.rs) keeps the library network-free (it fails if a
library module imports `ureq`/`libc`/`std::net`).

- [`specs/`](specs/) — design specifications (living documents). Start at
  [`specs/README.md`](specs/README.md).
- `Makefile` — build / test / coverage / lint targets (`make help`).
- `.githooks/pre-commit` — runs the full `make check` gate (fmt + clippy + 100% coverage)
  + the 300-line code-file cap, on commit and on `bl close`.
- [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — the `make check` gate (run once,
  it is platform-independent) plus the portability matrix.
- [`.github/workflows/release-plz.yml`](.github/workflows/release-plz.yml) — release-plz versioning + publish;
  [`release-binaries.yml`](.github/workflows/release-binaries.yml) attaches prebuilt `bz` binaries.

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
(`tests/live_conformance.rs`) asks the real one: *does one canonical request
produce the same NORMALIZED event grammar across every provider this box can
authenticate to?* That is the whole point of brazen, so this is the test that
proves it end-to-end against live endpoints.

For the same proof **without** real keys — and so **in CI, on every platform** —
`tests/sim_conformance.rs` runs the real `bz` binary over the real HTTP transport
against a tiny loopback server (`FakeProvider`) that replays the golden wire
fixtures. It asserts the normalized grammar for all five providers and that an HTTP
`401` maps to exit 77, catching `ureq`-round-trip and status-mapping defects the
in-process `MockTransport` cannot. Not `#[ignore]`d — it runs in plain `cargo test`.

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

Where the conformance suite drives the *one* happy path, `tests/live_fuzz_openai.rs`
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

### OpenAI ChatGPT-SSO OAuth circuit

`tests/live_oauth_openai.rs` (**bl-0272**) covers the *auth* half the fuzz suite
scoped but left out: it manipulates the stored credential to drive brazen's three
OAuth circuits (auth §6) against the live `openai-chatgpt` codex backend. Same
`#[ignore]`d, `BRAZEN_LIVE`-gated, coverage-excluded shape; skips (printed) without a
`bz login openai-chatgpt` cred.

- **`revoked-access` → 77** — a fresh-expiry cred with a bad *access* token: brazen
  skips refresh and sends the bad bearer → codex `401` → `from_http_status(401)=Auth`
  → exit **77** (the `401/403→Auth` mapping the fuzz suite's all-`400` matrix never
  reached live).
- **`revoked-refresh` → 77** — an expired cred with a bad *refresh* token: brazen
  refreshes → the token endpoint answers `invalid_grant` → exit **77**.
- **`silent-refresh` → 0** — an expired cred with the *real* refresh token: brazen
  mints a new access token over the token endpoint, persists it, and completes `200`;
  the test asserts the persisted token changed and its `expires_at` is in the future
  (the codex `jwt_exp` no-`expires_in` path, auth §10.3).

The two revoked circuits run on a **throwaway temp `XDG_DATA_HOME`** with synthetic
tokens, so the real refresh token is never sent — near-free (rejected before
generation). `silent-refresh` *must* send the real refresh token (OpenAI **rotates**
it on use), so it forces refresh on the **real** store and keeps brazen's persisted
result — a normal early refresh, leaving the credential valid — and is therefore both
token-costing and behind the second opt-in, `BRAZEN_LIVE_FUZZ_SPEND=1`.

```sh
BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
  cargo test -p bz --test live_oauth_openai -- --ignored --nocapture
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

brazen is **one crate** — `cargo install brazen` builds the `bz` command (the
`balls`→`bl` pattern) — and releasing is automated with
[release-plz](https://release-plz.dev) (`.github/workflows/release-plz.yml`):

- Every push to `main` refreshes a **release PR** that bumps the version in
  `Cargo.toml` and writes the `CHANGELOG` from the conventional-commit history.
  **Pushing never publishes** — it only maintains that PR.
- **Publishing is a deliberate manual step** (the "click release"): when ready, run
  the workflow by hand — *Actions → Release-plz → Run workflow* on `main`. Only that
  manual run publishes the current version to crates.io, tags it `v<version>`, and
  cuts a GitHub Release. The Release triggers `release-binaries.yml`, which builds
  the `bz` binary for every supported target and attaches the archives — so users
  without a Rust toolchain can grab a prebuilt `bz` (`bz-<target>.tar.gz` / `.zip`)
  instead of `cargo install`.

The `make check` gate (fmt + clippy + 100% coverage) runs on every push to `main`
via `ci.yml`, so what release-plz publishes is gated code.

**One-time setup** — repo *Settings → Secrets and variables → Actions*:

- `CARGO_REGISTRY_TOKEN` — a crates.io API token (publish scope) owned by the crate
  owner. Required.
- `RELEASE_PLZ_TOKEN` — recommended: a fine-grained PAT (or GitHub App token) so the
  release PR's commits re-trigger CI; falls back to the default `GITHUB_TOKEN`.

## License

MIT — see [`LICENSE`](LICENSE).
