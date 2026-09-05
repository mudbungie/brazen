# brazen

**`brazen`** (the **`bz`** command) — a stateless, swiss-army-knife adapter for every LLM
provider and protocol. Pipe a request in, stream a normalized response out.

One small Rust binary that speaks OpenAI `chat/completions`, OpenAI `responses`,
Anthropic `messages`, and Google `generative-ai` across providers (OpenAI, Anthropic,
Mistral, Google, local Ollama, …), handling API-key and OAuth/SSO auth. It is a low-level
building block for agents.

> **The namesake.** Medieval legend gave Roger Bacon, Albertus Magnus, and Pope
> Sylvester II each a *brazen head*: a cast bronze skull that answered any question put
> to it. The brass knew nothing on its own. It was a vessel — its makers were said to
> have bound a spirit into the metal, and the head spoke with the voice of whatever
> answered from the other side. That was the scandal, and the reason the tales end
> badly: you could ask, but you never owned the thing that replied.
>
> `bz` is the brass. The spirits are somebody else's, they are many, and they do not
> speak the same tongue — so the head does the translating: one shape of question in,
> one shape of answer out, whichever of them you happen to be calling.

## Install

```sh
cargo install brazen            # builds and installs the `bz` command
```

Or download a prebuilt `bz` for your platform from the [latest release][releases] — no Rust
toolchain required. Building from source needs Rust 1.85+.

[releases]: https://github.com/mudbungie/brazen/releases/latest

### Corporate roots / TLS-inspecting proxies — the `native-certs` feature

By default `bz` trusts a **bundled Mozilla root set** (compiled in via `rustls` +
`webpki-roots`), so a single static binary verifies public-CA certificates with no OS
trust store. That is the secure default, but it means a **private/corporate root CA** — or
a TLS-inspecting proxy's MITM root, which lives only in your OS trust store — is **not
trusted**, and such a connection fails the handshake (the error now names the cause, e.g.
`HTTP transport: io: invalid peer certificate: UnknownIssuer`). If you are behind one, build
from source with the **`native-certs`** feature, which trusts your OS certificate store
instead:

```sh
cargo install brazen --features native-certs
```

It is a build-time property (no runtime flag), OFF by default so the shipped binary's trust
set never silently widens to a host's.

## Quickstart

```sh
# one-shot: key on the env, model picked by --model (which prefix-routes to its provider)
ANTHROPIC_API_KEY=sk-ant-... bz --model claude-sonnet-4-6 "What is the capital of France?"
```

Set a default model once and the prompt is all you need — the brazen head speaks the answer:

```sh
export ANTHROPIC_API_KEY=sk-ant-...     # or BRAZEN_API_KEY; or `bz --login` for OAuth/SSO
export BRAZEN_MODEL=claude-sonnet-4-6
bz "What is the capital of France?"
bz "Summarize this: $(cat notes.txt)"     # feed data via the prompt (a positional prompt
                                          # overrides stdin; pipe a canonical JSON request with no arg)
bz -f notes.txt "Summarize this"          # or attach the file as context (repeatable)
bz -f photo.png "What is this?"           # images/PDFs attach by extension (png/jpg/jpeg/gif/webp/pdf)
bz "Name one dish" | bz -f - "How do I make this?"   # `-` names stdin, so runs chain
```

`-f -` is the only way stdin reaches a run that has a positional prompt — a bare
`bz "prompt"` still never reads stdin, so `bz` will not swallow the input of a
`while read … done < file` loop the way `ssh` does without `-n`.

With **nothing** specified — no `--provider`, no `--model`, no `BRAZEN_MODEL` — `bz` falls
back to the **first provider you declare** in the config (the first `[[provider]]` block,
top of file) and, for that provider, the **model you last used** with it — falling back to
its **first cached model** if you never have:

```sh
bz --list-models        # once: populate the default provider's model cache
bz "What is the capital of France?"   # zero-config: first-declared provider, last-used model
```

The cache also **learns from success**: any `2xx` records which model it used (that is the
last-used above), and if the cache couldn't yet place that model it is appended. So a single
`bz --provider X --model some-model "hi"` seeds the cache, and the next bare `bz "…"` defaults
to it — even for a provider whose `--list-models` endpoint is broken or you never ran. (It
records only the model *you* chose and the provider accepted; it never lists behind your back.)

The default is the first row *you* write, not the alphabetically-first — the built-in
providers brazen ships (anthropic, openai, …) sit below your rows, so they never hijack the
default. (`--model` and `--provider` are pure overrides; name a model and it routes by its
family, name a provider and it wins outright.)

More:

```sh
bz --login --provider openai-chatgpt --browser   # OAuth / Sign in with ChatGPT — no API key
bz --provider openai --model gpt-5 "explain monads in one line"
bz --list-providers                              # every provider bz can route to, and whether it can reach it
bz --list-models --provider anthropic            # discover the model ids a provider serves
bz --list-models --provider google --json        # …with provider-reported metadata (context_window etc.) where served
bz --json "..."                                  # canonical NDJSON event stream instead of text
bz --skill                                       # the fuller skill doc (worked examples) — richer than --help
```

## What works today

**Early implementation** — we design first (specifications in [`specs/`](specs/)), implement
second — but the core vertical slice is in and tested end-to-end:

- **Protocols** — OpenAI `chat/completions`, OpenAI `responses` (ChatGPT/Codex), Anthropic
  `messages`, Google `generative-ai`, Ollama (NDJSON), and the Claude Code CLI's
  `stream-json` (`specs/claude-code.md` — a subprocess, not HTTP), all normalized to one
  canonical request + `Event` stream. An executable single-source-of-truth test proves the
  five HTTP basic fixtures decode to the *same* `Vec<Event>`.
- **Providers** — OpenAI (api key, and `openai-chatgpt` for subscription sign-in — the one
  built-in OAuth row, so `bz --login … --browser` works with no config), Anthropic, Mistral,
  Google, local Ollama, and `claude-code`
  (the installed `claude` CLI driven as a pure model pass-through — an Anthropic-family
  path with **no API key**: `bz --provider claude-code -m sonnet "hi"` rides claude's own
  OAuth), added as config rows. **`claude-code` is deliberately single-turn, text-only,
  and tool-free** (`specs/claude-code.md` §4): a request carrying tool declarations,
  assistant history, or media **rejects at encode** (`parse_input`, exit 64) — the CLI's
  print mode cannot carry them, and a strip would silently change semantics. **That
  decline is published, not discovered at call time**: `bz --list-providers` carries a
  `shapes` column (`tools`, `multi_turn`, `-` for neither), so a host picking a row for
  a tool-bearing role refuses this one before it spends a call. Agentic
  callers (harnesses that declare tools or replay transcripts) need an HTTP
  `anthropic_messages` row instead; the same logged-in claude credential works there via
  an `ambient = { format = "claude_code", … }` recipe (`specs/auth.md`). Mistral is the
  severability floor: **one row, zero Rust** (it reuses the OpenAI dialect verbatim).
- **Auth** — API key (`x-api-key` or `Authorization: Bearer`, chosen by row data), keyless
  (`none`, for local Ollama), and OAuth2 / SSO with silent refresh, including **Sign in with
  ChatGPT** via `bz --login`.
- **Routing** — a model owns its provider by an exact alias or a prefix family (`claude-`,
  `gpt-`, …), so `--provider` is droppable for a model some row claims. Rows are a
  priority list in config order and the first owner wins. A model **no** row claims falls
  through to the first row whose **cached model list** matches it (`bz --model 5.5` skips
  the provider that has no 5.5) — a local read, never a probe, and a claim always outranks
  a cache match; missing/unknown providers surface as a clean config error.
- **Output** — streamed text (default), `--thinking`, `--json` (canonical NDJSON events), and
  `--raw` (lossless passthrough). `--raw` is **directional**: bare `--raw` (= `--raw=both`) is
  verbatim in **and** out; `--raw=in` sends the request verbatim but emits canonical events;
  `--raw=out` builds the request from `bz`'s ergonomics (prompt, `-f`, config, model cache,
  auth) and streams the provider's **exact wire bytes** back. A full sysexits-style exit table
  (0 / 64 / 66 / 69 / 70 / 77 / 78) and `BrokenPipe` -> 141.
- **Config** — one schema folded **flags > env > file > built-in defaults**, merged
  per-provider-row and per-field: declaring your own `[[provider]]` rows **patches** the
  built-in table, it never replaces it, so a row a later brazen ships still reaches you
  unedited. `--dump-config` prints that merge **minus the built-in floor** (dumping it
  would pin today's defaults in your file forever), secrets redacted — so a dump listing
  only your rows is the delta, not the effective table; `--list-providers` is the read of
  that effective table (name / protocol / auth / tuning / credential, in routing-priority
  order, built-ins included, zero round-trips) — where `tuning` names the request knobs
  the row accepts (`effort`, `priority`), computed from the dialect's projection and the
  row's own `unsupported_body_keys`. `--base-url <url>` / `BRAZEN_BASE_URL`
  points a run at a custom endpoint (local proxy, mock, vLLM, tenant gateway) — same
  provider, different host — with no temp config file.
- **Provider + model discovery** — `bz --list-providers` (offline: the effective row table,
  including each row's computed capability — which tuning knobs it accepts, which request
  shapes its dialect can carry, which headless sign-in it serves) and `bz --list-models`
  (one GET, over a lazy live-probe cache).
- **Ingress (masquerade)** — `bz --serve` runs an OpenAI-compatible AND an
  Anthropic-compatible HTTP endpoint in front of ANY configured provider: a harness that only
  speaks `chat/completions` — or an Anthropic SDK POSTing `/v1/messages` — points its
  `base_url` at brazen and reaches Anthropic, Google, Ollama, OpenAI, … The path picks the
  dialect (`POST /v1/chat/completions` vs `POST /v1/messages`; no extra config);
  `GET /v1/models` serves the local model cache plus every row's `model_aliases` keys.
  `bz --in openai_chat` (or `anthropic_messages`) is the same capability as a one-shot POSIX
  filter: one dialect request on stdin, the dialect response on stdout (SSE if the request
  says `stream:true`). A
  fail-open replay stash carries opaque reasoning payloads (thinking signatures,
  `encrypted_content`) across turns the client's dialect cannot; a stash miss degrades the
  turn and is exposed as a named adaptation (`"brazen":{"adaptations":[…]}` / an SSE comment),
  or rejected via `[ingress] lossy_overrides`.
- **Token counting** — `bz --count-tokens` returns a provider-accurate `input_tokens` for a
  request (one round-trip to the provider's count endpoint; Anthropic + Google, others decline
  with a config error rather than fabricate an estimate).
- **Transport** — a blocking, rustls-backed `ureq` client (no OpenSSL, no async runtime) with a
  single config-driven `--timeout` (the silence budget: abort when the upstream sends no bytes for
  N seconds, applied per phase — connect / headers / between chunks; not a wall-clock total).
  A row can also **hand its whole HTTP round-trip to a program you supply**
  (`specs/transport.md`) — because no header or config edit can make ureq/rustls look on the
  wire like a client built on another runtime, and brazen ships no impersonation profile:

  ```toml
  [[provider]]
  name = "my-adapter"
  base_url = "https://api.example.com"
  protocol = "anthropic_messages"
  auth = "api_key"
  api_header = { name = "x-api-key", scheme = "raw" }

    [provider.transport]
    program = "/opt/my-adapter/http-relay"    # yours; brazen never inspects it
  ```

  Your program gets one whole HTTP/1.1 request message on stdin (absolute-form target, brazen's
  headers verbatim, body framed by EOF) and answers with one whole response message on stdout,
  streamed. It owns the generated headers, framing, ALPN and TLS ClientHello; brazen keeps
  encode/auth/decode, the exit codes, `retry-after`, and the one-request-no-retry guarantee.
  Credentials ride the pipe only — never argv, env or a temp file. See
  [`examples/stdio_transport.rs`](examples/stdio_transport.rs) for a working delegate in ~100
  lines. Rows without the block are untouched.

The pure library is held at **100% line coverage**; the data plane is smoke-tested live against
Anthropic and OpenAI. The full design lives in [`specs/architecture.md`](specs/architecture.md).

## Serving the masquerade (`--serve` / `--in`)

Point an OpenAI-only harness at any provider brazen speaks. Config:

```toml
[ingress]                      # the deliberate opt-in; the route path picks the codec
# listen = "127.0.0.1:4891"    # default; non-loopback REFUSES to start without `token`
# token  = "..."               # optional bearer; set -> requests without it get 401

[[provider]]
name = "anthropic"
model_aliases = { "gpt-4o" = "claude-sonnet-4-6" }   # routes AND substitutes
```

The built-in `openai` row also claims `gpt-4o` (by its `gpt-` prefix), but your row is
declared first and the **first owner in config order wins** — so this one line diverts
`gpt-4o` to Claude while `openai` keeps serving every other `gpt-…`. Order decides, and
nothing warns you when it decides against you: `--dump-config` prints **your** rows in
order (the built-in ones stay in `data/defaults.toml`, beneath), `bz --list-providers`
prints the merged table in the order routing reads it, and `--provider` overrides
routing outright.

Then `bz --serve` — the harness sets `base_url = "http://127.0.0.1:4891/v1"` and keeps
sending `gpt-4o`; brazen decodes the request at the edge, runs the ordinary pipeline
against the routed provider (row auth, model cache, everything), and re-encodes the
canonical events as `chat.completion(.chunk)` — the client's `stream` field picks SSE vs
one JSON body, independently of the upstream's own streaming. The same listener also
answers the **native Anthropic route**: an Anthropic SDK sets
`base_url = "http://127.0.0.1:4891"` and its `POST /v1/messages` selects the
`anthropic_messages` codec by path — no config change; both ecosystems are served at
once, and errors on that surface wear Anthropic's `{"type":"error",…}` envelope.
`GET /v1/models` answers from the local model cache plus every row's alias keys (refresh
with `bz --list-models`), always in OpenAI's list shape; upstream errors come back in the
client dialect's error envelope with the real status. SIGINT/SIGTERM stops the listener.
The same edge works without a listener as a POSIX filter — no `[ingress]` table needed:

```sh
echo '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}' | bz --in openai_chat
```

Multi-turn reasoning survives the dialect: opaque replay payloads (Anthropic thinking
signatures, `encrypted_content`, Google `thoughtSignature`) park in a fail-open XDG stash
(`$XDG_CACHE_HOME/brazen/replay/`) and are re-injected when the client echoes the turn
back. A stash miss never breaks the turn — brazen omits thinking for that replay turn and
says so (`"brazen":{"adaptations":["thinking_replay"]}` on aggregates, a `: brazen
adaptation=…` SSE comment on streams); set `[ingress] lossy_overrides = { thinking_replay
= "reject" }` to refuse instead. Full design: [`specs/ingress.md`](specs/ingress.md).

## Sign in with ChatGPT (OpenAI SSO)

`bz` can authenticate against a ChatGPT subscription using the same OAuth flow the Codex CLI uses.
**This one ships built-in** — no config file, nothing to paste:

```
bz --login --provider openai-chatgpt --browser
```

That opens the ChatGPT consent page, captures the loopback redirect, and stores the credential.
Afterwards `bz --provider openai-chatgpt --model gpt-5.4 "hi"` runs against the subscription, with
the token refreshed silently.

`bz --login --provider <id>` has two flows: the **default** is the headless **device flow** (it prints a
short code to enter on another device — needs no local browser, ideal over SSH); **`--browser`**
runs the loopback browser flow (it opens the authorize URL and captures the redirect) when the
provider's registered redirect is a loopback URL, as the ChatGPT row is. Both end in one
stored credential. Run `bz --login --help` for the synopsis.

The ChatGPT row serves **both**. Its `device` block declares OpenAI's own device-code wire, which is
not RFC 8628 — hence `style = "codex"` — so `bz --login --provider openai-chatgpt` with no `--browser`
prints a code and a URL you can open on any device, a phone included, while `bz` polls on the machine
you ran it on. That is the flow to use when `bz` runs somewhere you are not sitting: the browser flow
needs a browser that can reach *that machine's* loopback. One caveat comes from the vendor, not from
brazen: device-code login can be switched off for a ChatGPT account or workspace in its security
settings, and a workspace with it off refuses the flow — brazen prints the provider's own refusal
verbatim. Use `--browser` there.

Which flows a row serves is readable without trying one: `bz --list-providers` carries a `device`
column naming the wire (`codex`, `rfc8628`, or `-` for a row that can only be signed in with
`--browser`).

This is the **only** built-in OAuth row. Every other provider row ships api-key/bearer, and the core
still compiles in no vendor login policy — the row below is pure data in the embedded table
(`data/defaults.toml`), reproduced here so you can see what it declares, override a field, or model
your own row on it. Deleting it would delete the capability and no Rust with it.

```toml
[[provider]]
name       = "openai-chatgpt"
base_url   = "https://chatgpt.com/backend-api/codex"
protocol   = "openai_responses"
auth       = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
# Canonical request-body fields this backend REJECTS — the inverse of body_defaults.
# brazen strips each before encoding, so a stray --temperature/--top-p/--max-tokens
# never reaches the wire (the Codex backend 400s on all three; see specs/config.md §4.1.1).
# A TOP-LEVEL row key: it must precede the [provider.…] sub-tables, or TOML reads it as
# a member of the last one opened rather than as a field of the row.
unsupported_body_keys = ["max_tokens", "temperature", "top_p"]

[provider.oauth]
authorize_url    = "https://auth.openai.com/oauth/authorize"
token_url        = "https://auth.openai.com/oauth/token"
client_id        = "app_EMoamEEZ73f0CkXaXp7hrann"
scope            = "openid profile email offline_access api.connectors.read api.connectors.invoke"
redirect         = { host = "localhost", port = 1455, path = "/auth/callback" }
device           = { url = "https://auth.openai.com", style = "codex" }   # the headless flow: the AUTH BASE, whose /deviceauth/* and /codex/device it derives
authorize_params = [["id_token_add_organizations", "true"], ["codex_cli_simplified_flow", "true"], ["originator", "codex_cli_rs"]]
account_header   = "ChatGPT-Account-ID"
beta_headers     = [["originator", "codex_cli_rs"]]

[provider.body_defaults]   # request-body fields this backend always needs
store  = false             # the Codex backend 400s unless store:false

[provider.models]          # the --list-models override; this backend's /models is not the protocol default
path      = "/models"
query     = [["client_version", "0.0.0"]]   # /models 400s without it; the sentinel returns the full catalog
array_key = "models"                        # {"models":[…]}, not the protocol-default {"data":[…]}
id_key    = "slug"                          # each entry's id is `slug`, not `id`
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

It reaches a **nested** wire field too: where a dialect nests its generation params, an object
here merges into the object `bz` builds rather than losing to it, key by key. That is the one way
to state Ollama's **context window**, which has no canonical field (`num_predict` caps the output;
`num_ctx` sizes the input, and `bz` never invents one — without a pin, the request runs at whatever
the Ollama server defaults to, which a long tool-carrying prompt will overrun):

```toml
[[provider]]
name = "ollama"
base_url = "http://localhost:11434"
protocol = "ollama_chat"
auth = "none"
body_defaults = { options = { num_ctx = 32768 } }   # composes with the typed max_tokens
```

A pinned `num_ctx` is also what the turn's `usage` events report as their
`context_window` — the number the server will actually allocate, so it is the honest
denominator for the counters beside it.

### Stating a context window a provider does not serve

`context_windows` declares, per model, the input token limit the `usage` events should
carry as their `context_window` (`specs/model-discovery.md` §5.5). Most providers'
`/models` GET serves no limit at all — Anthropic's, OpenAI's and Ollama's do not — so
without this the denominator for context-fullness metering is dark on nearly every turn
and every harness above `bz` keeps its own model table to divide by:

```toml
[[provider]]
name = "anthropic"
context_windows = { "claude-opus-4-1-20250805" = 200000 }
```

Keys are **wire model ids**, the id the request actually carries. A served window (where
a provider publishes one) wins over a declaration; a model neither serves nor declares
carries no key at all, never a fabricated number.

`unsupported_body_keys` is the **inverse** of `body_defaults`: where `body_defaults` *fills* a
field the backend always needs, `unsupported_body_keys` *strips* a field the backend cannot accept.
The Codex backend 400s on `temperature`, `top_p`, and `max_output_tokens` with
`{"detail":"Unsupported parameter: …"}` (validated live 2026-06-17) — `bz` renames
`max_tokens`→`max_output_tokens` per the Responses spec, but this one backend forbids the standard
sampling/length params. With the three keys listed above, `bz` silently drops them before encoding
(naming **canonical** fields — `max_tokens`, not the wire `max_output_tokens` — so the rename stays
owned by the encoder), so passing `--max-tokens`/`--temperature`/`--top-p` (or the same keys in the
input JSON) against this row no longer 400s — the value is normalized away, the request streams
normally. brazen now supplies or normalizes every one of this backend's quirks (`store:false`,
`stream:true`, and the three rejected params); none is left for the operator to honor by hand.
(A fourth quirk, a mandatory non-empty `instructions`, **lapsed on the service side** — re-probed
2026-07-31, a body with no `instructions` now completes normally; see `specs/auth.md` §10.7.)

The flow, the verified Codex wire facts behind each field, and the empirical risks still to confirm
end-to-end (e.g. the data-plane request shape against the `codex` backend) are documented in
[`specs/auth.md` §10](specs/auth.md).

## OAuth providers in general (and a note on Anthropic)

The OAuth machinery is **vendor-blind** and reachable by config: a provider row with
`auth = "oauth2"` resolves like any other, given a `[provider.oauth]` block of operator-supplied
values. Nothing about any specific vendor is compiled in — brazen bakes in **no** vendor login
policy, and the one OAuth row it ships (`openai-chatgpt`, above) is data in the embedded table like
every other row, not code ([`specs/auth.md` §7, §10.5](specs/auth.md),
[architecture.md §13](specs/architecture.md) item 3). Add your own the same way; the fields the row
understands, all data:

```toml
[[provider]]
name       = "my-oauth"        # an ALTERNATE row; claims no model_prefixes ⇒ reach it via --provider
base_url   = "https://…"
protocol   = "anthropic_messages"   # or openai_responses / openai_chat / …
generation_query = [["preview", "true"]] # optional generation-POST query; protocol still owns the path
auth       = "oauth2"          # silent refresh for brazen-owned credentials
api_header = { name = "Authorization", scheme = "bearer" }

[provider.oauth]
authorize_url   = "https://…/authorize"   # operator-supplied; nothing vendor-specific is compiled in
token_url       = "https://…/token"       # token exchange AND silent refresh
client_id       = "…"
scope           = "…"
beta_headers    = [["…", "…"]]            # auth-mode-DEPENDENT headers, sent ONLY under OAuth (auth §4)
system_preamble = "…"                     # text the request's system must LEAD with, prepended in resolution (auth §4.1)
```

A row may also carry an `ambient` block to **borrow** a credential another tool already wrote
(see [`specs/auth.md`](specs/auth.md) §5.5, *Ambient credential discovery*). A fresh borrowed
OAuth token authenticates the request; an expired one returns 77 and must be refreshed by its
owner — brazen never refreshes or copies foreign state into its store. `bz --login --provider <id>
--browser` runs the loopback flow for credentials brazen owns when the vendor's registered redirect
is a loopback URL. See
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
- [`SKILL.md`](SKILL.md) — the agent-facing skill card `bz --skill` prints (compiled into
  the binary via `include_str!`). Read it directly, or drop it into an agent's context as-is.
- `Makefile` — build / test / coverage / lint targets (`make help`).
- `.githooks/pre-commit` — runs the full `make check` gate (fmt + clippy + 100% coverage)
  + the 300-line code-file cap, on commit and on `bl close`.
- `.githooks/reference-transaction` — whenever local `main` advances (the gate having
  passed), pushes it to origin and installs `bz` from that tip (`make install`, detached,
  from a worktree at the new ref). See [`AGENTS.md`](AGENTS.md) "Close gates".
- [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — the `make check` gate (run once,
  it is platform-independent) plus the portability matrix.
- [`.github/workflows/release-plz.yml`](.github/workflows/release-plz.yml) — release-plz versioning +
  publish, plus the binaries job that attaches prebuilt `bz` archives in the same run. The process
  around it (the test ladder, the release gate, the human steps) is [`specs/release.md`](specs/release.md).

## Embedding (shelling out vs. linking the library)

A harness has two ways to reach a provider through brazen, and they cost differently.

- **Shell out to `bz`** — the simple, fully-supported default. Each call is one process:
  a spawn, a fresh TLS handshake, a re-parse of the embedded defaults, a config-file read,
  and (on generation) a model-cache read — none of it reused between calls, because a
  subprocess has nowhere to keep a connection pool, so HTTP keep-alive is *structurally
  unavailable*. Against a multi-second generation this overhead is noise; it only matters at
  high call frequency with short completions. For concurrency you spawn N processes — brazen
  never fans out in-process (that is the harness's job to schedule and reap).
- **Link the library** (`brazen` as a crate dependency) — the sanctioned path when the
  per-call overhead actually bites. Construct one `HttpTransport` (`HttpTransport::new`) and
  **hold it across calls**: the lone `ureq::Agent` — connection pool and all — lives on that
  struct, so calling `generate` repeatedly through the same transport reuses the kept-alive
  connection, plus the already-parsed config and the warm model cache, all in-process. You
  consume the typed `Event` stream directly instead of parsing bytes.

There is **no call-mechanics daemon**, by design — improving call mechanics belongs to a
library embedder, not a long-running server inside `bz` (which would grow the stateful,
connection-owning surface the stateless model deliberately refuses). `bz --serve` is not
that daemon: it exists to translate *dialects* (the masquerade edge above), each request an
independent stateless pipeline, not to amortize per-call overhead. The library API is not yet
a stability contract (pin an exact version); see [`specs/architecture.md`](specs/architecture.md)
§12 for the full cost accounting.

## Build

```sh
make hooks   # one-time per clone: enable the pre-commit gate + main-advance hook
make check   # fmt + clippy + 100% coverage gate
make install # install this tree's bz into ~/.cargo/bin (the hook runs this on every merge to main)
make smoke   # live request per provider (real keys; skips providers whose key is unset)
make release-check  # the release gate: make check + every live suite, one roster
```

`make release-check` is the ladder's **release rung** ([`specs/release.md`](specs/release.md)):
a human runs it on a credentialed workstation against `main`'s tip immediately before merging
the release PR, and pastes the roster it prints into that PR. It sets no environment — each
suite self-gates on the spelling its own module doc names (`BRAZEN_LIVE`,
`BRAZEN_LIVE_FUZZ_SPEND`, `OLLAMA_SMOKE`, the provider key vars) — and a suite that gated
itself off is a loud SKIP, never a pass. It exits non-zero on any suite failure **and** when
nothing ran at all, so a credential-less box can never print a green release gate.

It classifies nothing. A live case whose truth is the model's choice rather than the wire
dialect's — a reasoning summary the model may skip — declares itself
`Determinism::Discretion` on the case (`tests/live_support/determinism.rs`), and the suite
re-runs that case up to three times itself before reporting anything. The gate only quotes
those lines back into the roster, so there is no second list of "flaky" cases to drift.

## Live conformance suite

`make smoke` (`scripts/smoke.sh`) asks shallow questions — *did each provider with a key
return exit 0 + non-empty output on a good key (keeping `--json`/`--raw` output-mode shape),
and a correct non-zero exit + a non-empty surfaced provider error on a bad one?* It also probes the
**OAuth2 / SSO data plane** (bl-61a6): the real `AuthId::OAuth2` path via a stored `bz --login
--provider openai-chatgpt` cred, and the anthropic Max OAuth token (`sk-ant-oat01…`) through a bearer +
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
  cargo test -p brazen --test live_conformance -- --ignored --nocapture
```

**Providers are discovered at runtime.** For each row the harness looks for a
usable credential — a reachable keyless endpoint (Ollama), a stored `Cred` from
`bz --login --provider <id>` (e.g. OpenAI "Sign in with ChatGPT"), or one of the row's
API-key env vars — and **skips, never fails,** any provider with none, printing
the reason (no silent truncation). A box with zero credentials is a clean no-op.

**Per authed provider it asserts the canonical surface:** the streamed-text event
grammar over `--json` (`message_start` → text `content_start` → `text_delta` →
`usage` → `finish` → terminal `end`), the `--text` projection, system/instructions
(every request carries a non-empty `system`), a tool round-trip where the row
supports it (a `tool_use` `content_start` + streamed `json_delta` arguments), and
error mapping (a deliberately bad model → exit 69), and the `--raw` projection
(lossless passthrough → exit 0 + non-empty native wire bytes; `--raw` sends stdin
verbatim, so the harness feeds each row its *native*-shaped body — the canonical
messages shape for messages-dialects, `contents` for Google, `/api/chat` for
Ollama — declared per row as `raw: RawBody::…`, bl-5f6e). The raw path is
implemented and **offline-tested** (`run_cache.rs`, `seams_protocol.rs`, the sim
suite); the live harness now exercises it on the wire too.

**Adding a provider is one `Row`** in the `TABLE` (no code branches — quirks are
DATA): set `provider`/`model`/`model_env`, the `auth` discovery strategy
(`Keyless { probe }` or `Keyed { env }`), and the per-row knobs (`max_tokens:
None` to omit it, `store_false`, `tools`, the `raw` body shape). The harness drives the same assertions
for every row. (The codex backend's quirks — no `max_output_tokens`, explicit
`store:false` — live entirely in its row as data, validated live 2026-06-16. A
non-empty `instructions` used to be a third; the service dropped that mandate by
2026-07-31, so the row's `store_false`/`max_tokens: None` data is what still matters.)

### OpenAI ChatGPT-SSO fuzz

Where the conformance suite drives the *one* happy path, `tests/live_fuzz_openai.rs`
(**bl-b72f**) drives a *wide range of request shapes* at the live `openai-chatgpt`
codex backend — surfacing where brazen mis-encodes or mis-maps errors. It reuses the
conformance harness leaves (`live_support/exec.rs`, `…/grammar.rs`) verbatim, so it is
the same black-box, `#[ignore]`d, `BRAZEN_LIVE`-gated, coverage-excluded shape, and
skips (printed reason) without a `bz --login --provider openai-chatgpt` cred. Two families:

> **The provider row must be named `openai-chatgpt`** — every live OpenAI suite here
> hardcodes that name (`tests/live_support/openai.rs: PROVIDER`) and looks for a cred at
> `$XDG_DATA_HOME/brazen/credentials/openai-chatgpt.json`. A working credential filed
> under a *different* row name (e.g. `codex`) makes the whole suite SKIP while `bz` itself
> runs fine — the machine conforms to the suite, not the other way round (**bl-365f**
> owner ruling; no env knob). Rename the row in `~/.config/brazen/config.toml` **and** the
> credential file — the two are keyed together.

- **Error-conformance matrix** — request shapes the codex backend must 400 → exit 69
  **and** surface the service's own message for, asserted end-to-end (the codex
  `{"detail":…}` body reaching the `CanonicalError` is what **bl-5fe6** fixed; an empty
  message here is a regression). These 400 before generation, so they are ~free.
  A mandate earns a case here only if brazen's **canonical path can actually violate
  it**: the row pins `store:false` via `body_defaults` and `serve` forces `stream:true`,
  so a "no `store`" / "`stream:false`" case would assert brazen's own normalization
  while reading as a codex tripwire — both are excluded and re-probed by hand with
  `--raw` instead (still in force, 2026-07-31). What remains is the unsupported
  `gpt-5-codex` model → `"…not supported…"` (**bl-30b0**).
- **Request-shape acceptance** — well-formed variations (unicode/emoji content,
  multi-turn role ordering, a tool round-trip, a reasoning-summary run, the stripped
  sampling params) that must return exit 0 + the canonical grammar. A **body with no
  `instructions`** lives here too: it was an error case until the codex backend stopped
  requiring the field (2026-07-31), and the suite's drift policy *moves* such a case to
  the acceptance set rather than deleting it, so a silent re-imposition still fails loudly
  (**bl-30b0**, `specs/auth.md` §10.7). These GENERATE, so they are behind a SECOND
  opt-in, `BRAZEN_LIVE_FUZZ_SPEND=1`, and the run prints what ran vs was capped.

```sh
BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
  cargo test -p brazen --test live_fuzz_openai -- --ignored --nocapture
```

(Raw-SSE golden capture for offline-decoder replay is intentionally *not* duplicated
here: the offline `response.*` decoder is already exhaustively fixture-tested in the
in-crate `tests::responses_fixtures` / `tests::responses_decode_errors` modules
(`src/tests/`, arch §9.8), so this suite is the request/error conformance the offline
path structurally cannot reach.)

### OpenAI ChatGPT-SSO OAuth circuit

`tests/live_oauth_openai.rs` (**bl-0272**) covers the *auth* half the fuzz suite
scoped but left out: it manipulates the stored credential to drive brazen's three
OAuth circuits (auth §6) against the live `openai-chatgpt` codex backend. Same
`#[ignore]`d, `BRAZEN_LIVE`-gated, coverage-excluded shape; skips (printed) without a
`bz --login --provider openai-chatgpt` cred.

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
  cargo test -p brazen --test live_oauth_openai -- --ignored --nocapture
```

## Platform support

CI builds **and tests** the workspace on every target on a native runner — no
cross-emulation, so portability is proven by execution. The one exception is
`x86_64-apple-darwin`†, cross-built (build-verified) on the Apple-Silicon runner
because GitHub no longer offers a working Intel-mac runner.

| OS | x86_64 | aarch64 | static |
|---|---|---|---|
| Linux | `x86_64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu` | `x86_64-unknown-linux-musl` |
| macOS | `x86_64-apple-darwin`† | `aarch64-apple-darwin` | — |
| Windows | `x86_64-pc-windows-msvc` | `aarch64-pc-windows-msvc` | — |

† Built and shipped as a prebuilt binary, but not natively tested in CI (no
GitHub-hosted Intel-mac runner executes) — covered by the natively-tested
Apple-Silicon target plus the pure, portable codebase.

The matrix stays green because the native surface is deliberately tiny: **no system
C library, no OpenSSL, no `pkg-config`** to discover — that is what keeps
cross-compilation clean. (TLS is `rustls`; its `ring` crypto provider compiles and
*statically vendors* its own C/assembly, and brazen's own code has no build script,
C, or codegen.) There is no async runtime, and the `brazen`
lib has **zero platform-specific code** — the one OS branch (browser-launch argv)
lives behind the `BrowserLauncher` seam in the `bz` shim and is tested as data for
all three OSes. The single conditional dependency (`libc`, for restoring the Unix
SIGPIPE disposition) is `bz`-only and `[target.'cfg(unix)']`-gated.

### Windows secret-at-rest: documented limitation

Stored credentials are one JSON file per provider, written atomically (temp-file +
rename). On **Unix** the file is forced to mode **`0600`** at write time. On
**Windows** the file simply **inherits the user-profile directory ACL** — there is
**no DPAPI encryption and no explicit ACL hardening**. This is a deliberate v0.1
trade-off, *not* a code branch: adding DPAPI would pull in a Windows-specific
*system* C dependency and break the no-system-C, single-binary portability story
above. Treat
secrets on a shared or improperly-permissioned Windows profile as readable by other
accounts on that machine. (See architecture spec §6.4 / §10.)

## Releasing (publishing to crates.io)

brazen is **one crate** — `cargo install brazen` builds the `bz` command (the
`balls`→`bl` pattern) — and releasing is automated with
[release-plz](https://release-plz.dev) (`.github/workflows/release-plz.yml`):

- Every push to `main` refreshes a **release PR** that bumps the version in
  `Cargo.toml` and stages the next `CHANGELOG.md` entry. This repo's commit history
  is **not** conventional-commits, so `CHANGELOG.md` is **hand-curated** — you write
  the prose for each release (see the file's header); release-plz prepends the
  version bump. Pushing work never publishes — it only stages the next release.
- **The release PR merges itself on a green build** (`.github/workflows/release-automerge.yml`).
  Merging it was the last hand in the pipeline and is no longer one: the decision it asked
  for was made by the work that landed on `main`, and the publish is gated *after* the merge
  on the same CI verdict, so a red build simply leaves the bump sitting on `main`. The
  workflow waits on the `CI` workflow's own conclusion for the pull request's head commit,
  merges with `RELEASE_PLZ_TOKEN` (merging with the default `GITHUB_TOKEN` would trigger no
  CI run on `main` and so publish nothing), and prints a verdict line for every open pull
  request on every run. **To hold a release, mark the release PR a draft** — the workflow
  skips drafts, and un-drafting releases it on the next refresh.
- **Merging the release PR publishes — automatically, on a green build.** The merge
  triggers CI; when CI concludes successfully on `main`, the publish job (gated on
  that `workflow_run` success) ships the new version to crates.io, tags it
  `v<version>`, and cuts a GitHub Release. A binaries job in the **same run**
  (`needs:` the publish job, gated on its `releases_created` output) then builds the
  `bz` binary for every supported target and attaches the archives — so users without
  a Rust toolchain can grab a prebuilt `bz` (`bz-<target>.tar.gz` / `.zip`) instead of
  `cargo install`. It is folded in rather than triggered `on: release` because a
  Release created by the default `GITHUB_TOKEN` cannot start another workflow.

So the pipeline is **hands-off and build-gated**: nothing reaches crates.io unless
CI is green, and no step waits on a hand. The human process that used to surround the
merge — the live release gate (`make release-check`, above), changelog curation, the
version-bump rule, and post-publish artifact verification — is specified in
[`specs/release.md`](specs/release.md), whose §3 records what auto-merge changed about
when each of those runs. (*Actions →
Release-plz → Run workflow* remains a manual override.) `CARGO_REGISTRY_TOKEN` is
the enable switch — until it's set, the publish job has nothing it can ship; setting
it arms auto-publish, and the **first** release (`0.0.1`, already staged on `main`)
ships on the next green build.

The `make check` gate (fmt + clippy + 100% coverage) plus the cross-platform matrix
are part of that CI, so a published version is always gated, tested code.

**One-time setup:**

- **Let the release PR be opened.** *Settings → Actions → General → Workflow
  permissions* → enable **"Allow GitHub Actions to create and approve pull
  requests."** Without this (or a `RELEASE_PLZ_TOKEN` below), the `release-pr` job
  fails with `403 GitHub Actions is not permitted to create or approve pull
  requests` — the default `GITHUB_TOKEN` can't open PRs until you flip this.
- **`CARGO_REGISTRY_TOKEN`** (*Settings → Secrets and variables → Actions*) — a
  crates.io API token (publish scope) owned by the crate owner. Required to publish.
- **`RELEASE_PLZ_TOKEN`** — recommended, and **required for the auto-merge**: a
  fine-grained PAT (or GitHub App token) so the release PR's commits re-trigger CI (and
  which also satisfies the PR-creation permission above); falls back to the default
  `GITHUB_TOKEN` when unset, in which case the release PR carries no checks and
  `release-automerge.yml` merges nothing and says so.

## License

MIT — see [`LICENSE`](LICENSE).
