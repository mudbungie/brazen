# bz — a stateless LLM adapter (agent skill card)

`bz` is one small binary that speaks every LLM provider and protocol behind ONE
interface: pipe a request in, stream a normalized response out, get a POSIX exit
code. It is a building block for agents — stateless, no daemon, no session. The
only disk it touches is XDG config and credentials.

This card is the working reference: the input model, the output projections,
auth, config, the control operations, and the exit table — each with a command
you can copy. For the terse one-screen synopsis run `bz --help`; for the full
design see `specs/`.

## The one rule: how a request arrives

A request arrives EXACTLY one way — a positional PROMPT on argv **XOR** a
canonical request (JSON) on stdin. A positional prompt WINS and stdin is not
read. Options come before the prompt; the first bare word starts the prompt and
everything after it (including `--flags`) is prompt text.

```sh
bz "What is the capital of France?"                    # argv prompt
echo '{"messages":[{"role":"user","content":"hi"}]}' | bz   # canonical request on stdin
bz --json "hi"        # flags BEFORE the prompt
bz "hi" --json        # the prompt is:  hi --json   (a bare word ends option parsing)
bz -- --weird         # `--` ends options: the prompt is the literal  --weird
```

A leading bare word is ALWAYS a prompt — control operations are flags, never
verbs, so `bz "login"` is a prompt asking about login, not the login command.

## Output projections (`--text` / `--json` / `--raw`)

The response is projected by a flag; the default is plain streamed text.

```sh
bz "explain monads in one line"          # --text (default): human-readable, streamed
bz --thinking "prove sqrt 2 irrational"  # also surface reasoning/thinking output
bz --json "hi"                           # canonical NDJSON event stream (one event per line)
bz --no-stream "hi"                      # fold ONE JSON body instead of streaming
```

`--json` emits the canonical event grammar, one JSON object per line:
`message_start` → `content_start` → `text_delta`… → `usage` → `finish` → `end`.
Parse THIS, never the text skin, when scripting.

`--raw` is **directional** — verbatim provider-native bytes on the chosen axis:

```sh
bz --raw "hi"        # = --raw=both : request AND response verbatim, provider-native
bz --raw=in  "hi"    # send the stdin body verbatim, but emit canonical events out
bz --raw=out "hi"    # build the request from bz's ergonomics, stream the provider's
                     # EXACT wire bytes back (the encode-observability window)
```

## Choosing a model and provider

A model owns its provider by an exact alias or a prefix family (`claude-`,
`gpt-`, …), so `--provider` is droppable. Config row order is the priority list:
when several rows claim one model, the FIRST claiming row wins (never an error).
If NO row claims it, `bz` falls through to the first row whose CACHED model list
matches the seed — so `bz --model 5.5` skips a provider with no 5.5 and lands on
the one that has one (needs a prior `bz --list-models` for that provider; `bz`
never probes to route). A claim always beats a cache match, on any row.
Set a default once and the prompt is all you need:

```sh
export BRAZEN_MODEL=claude-sonnet-4-6
bz "hi"                                          # routed to anthropic by the claude- prefix
bz --model gpt-5 "hi"                            # routed to openai by gpt-
bz --model 5.5 "hi"                              # no row claims "5.5": falls
                                                 # through to whichever provider's
                                                 # cached list has a 5.5 model
bz --provider openai --model gpt-5 "hi"          # provider wins outright
bz --provider ollama --model llama3.2 "hi"       # a local, keyless provider
```

With NO model specified — no `--model`, no `BRAZEN_MODEL`, no config `model` —
`bz` walks a three-rung ladder for that provider: (1) configured model, if any;
(2) the model you LAST used successfully with it; (3) whatever its cached list
lists first. (With no `--provider` either, the provider is the FIRST row you
declare in config.) So `bz --provider anthropic "hi"` sends whatever you last
ran on anthropic, not the top of the list. The cache learns both halves: any
call that returns `2xx` points last-used at the model it used, and if that model
was not on the cached list it is appended (never reordered). `bz --list-models`
refreshes the list and leaves last-used alone.

## Auth: keys, env, and OAuth/SSO

Three auth modes, chosen by the provider row's data: API key, keyless (`none`,
for local Ollama), and OAuth2/SSO with silent refresh. A key is found on argv,
then the credential store, then the environment.

```sh
export ANTHROPIC_API_KEY=sk-ant-...       # provider-specific env var
bz "hi"
export BRAZEN_API_KEY=sk-...              # the generic override
bz --api-key sk-... "hi"                  # inline, highest precedence

bz --login --provider openai-chatgpt --browser   # OAuth: Sign in with ChatGPT (loopback flow)
bz --login --provider my-oauth                    # OAuth: headless device flow (prints a code)
```

`--login` is the ONE interactive surface; the data plane never enters it. The
default flow is the headless **device flow** (a code to enter on another device,
ideal over SSH); `--browser` is the loopback browser flow. Both end in one
stored credential and silent in-band refresh thereafter.

## Config: one schema, folded flags > env > file > defaults

Config lives at `$BRAZEN_CONFIG` or `$XDG_CONFIG_HOME/brazen/config.toml`.
Everything is one schema folded in precedence order; inspect the merge with:

```sh
bz --dump-config                          # print the merged config as TOML (secrets redacted)
bz --config ./my.toml --dump-config       # …from a specific file
bz --base-url http://localhost:8000/v1 "hi"   # same provider, different host (proxy/mock/vLLM)
```

A provider is a `[[provider]]` row — **data, not code**. Adding one, or an OAuth
provider, or a per-row body quirk, is a config edit:

```toml
[[provider]]
name = "openai"
model_prefixes = ["gpt-"]                 # the family this row claims
model_aliases  = { "gpt-4o" = "gpt-4o-2024-08-06" }   # routes AND substitutes
body_defaults  = { max_tokens = 4096 }    # per-row request-body defaults (lowest precedence)
unsupported_body_keys = ["top_p"]         # fields this backend rejects — stripped before the wire
```

The first row YOU write is the default (built-in rows sit below yours, never
hijacking it). Removing a provider deletes config, never core code.

## Control operations (each replaces the run, then exits)

```sh
bz --list-models --provider anthropic     # one GET: the provider's model ids
bz --list-models --provider google --json # …with provider metadata (context_window, …)
bz --count-tokens "hi"                     # provider-accurate input-token count (one round-trip)
bz --count-tokens --json "hi"              # {"input_tokens":N} instead of the bare N
bz --dump-config                           # the merged config as TOML
bz --help                                  # the one-screen synopsis
bz --version                               # the package version
bz --skill                                 # this document
```

The control ops (`--login` / `--list-models` / `--count-tokens` / `--dump-config`
/ `--serve`) are mutually exclusive. `--count-tokens` on a provider with no count
endpoint declines with exit 78 rather than fabricating an estimate.

## Attaching context and reading from files

```sh
bz --system "You are terse." "hi"          # a leading system prompt
bz -f notes.txt -f data.csv "summarize"    # attach file text (repeatable; before the prompt)
bz --input request.json                    # read the canonical request from a file, not stdin
bz --max-tokens 500 --temperature 0.2 --top-p 0.9 "hi"   # generation knobs
bz --reasoning high "hard problem"         # portable reasoning-effort knob (low|medium|high)
bz --timeout 30 "hi"                       # abort on 30s of upstream SILENCE (per phase, not total)
```

## The masquerade: serve OpenAI to any provider (`--serve` / `--in`)

Point an OpenAI-only harness at ANY provider. `--serve` runs an OpenAI-compatible
HTTP endpoint; `--in` is the same edge as a one-shot POSIX filter.

```sh
# One-shot filter — no [ingress] table needed:
echo '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}' | bz --in openai_chat

# Long-running endpoint (needs an [ingress] table naming the dialect):
bz --serve            # the harness sets base_url=http://127.0.0.1:4891/v1 and keeps sending gpt-4o
```

```toml
[ingress]
dialect = "openai_chat"     # required; the listener never sniffs
# listen = "127.0.0.1:4891" # default; non-loopback REFUSES to start without `token`
```

The client's `stream` field picks SSE vs one JSON body, independently of the
upstream. Opaque reasoning payloads (thinking signatures, `encrypted_content`)
park in a fail-open replay stash and re-inject across turns; a miss degrades the
turn and is named (`"brazen":{"adaptations":[…]}`), never silent.

## Exit codes (sysexits)

| code | meaning |
|------|---------|
| 0    | success — INCLUDING a provider refusal (a 200 that says no) |
| 64   | usage: bad/unknown flag, malformed stdin request |
| 66   | `--input`/`--file` missing or unreadable |
| 69   | transport error, upstream 4xx (incl. 429), premature EOF |
| 70   | upstream 5xx (retryable) |
| 77   | auth: 401/403, missing credentials, login/refresh failure |
| 78   | config: no/unknown provider or model, bad config |
| 130 / 141 / 143 | interrupted by signal (SIGINT / SIGPIPE / SIGTERM) |

An error after streaming begins is an in-band `Event::Error` on the stream, then
the terminal `end`, then the exit — so `--json` consumers see the failure as data.

## Worked recipe: one prompt, three providers, same grammar

```sh
export ANTHROPIC_API_KEY=sk-ant-...  OPENAI_API_KEY=sk-...
for m in claude-sonnet-4-6 gpt-5; do
  bz --model "$m" --json "Name one prime." | jq -r 'select(.type=="text_delta").text'
done
# Both routes normalize to the SAME canonical event grammar — that is the whole point of bz.
```
