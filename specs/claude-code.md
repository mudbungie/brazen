# Claude Code pass-through: the `claude-code` provider (exec transport + `claude_code` protocol)

> Derives from [architecture.md](architecture.md); must not contradict it. Empirical facts below
> were captured against the locally installed Claude Code CLI **v2.1.217** (2026-07-21); the
> committed fixtures are those real captures verbatim (§8).

---

## 1. Purpose & Scope

A shipped provider row, **`claude-code`**, that drives the locally installed Claude Code CLI
(`claude`) as a **pure model pass-through**: one canonical request in, one model response out,
with **every** native Claude Code behavior suppressed — no agentic loop, no tools, no
CLAUDE.md/settings context, no hooks/plugins/LSP/MCC/skills, no session persistence. The win is
an Anthropic-family data plane that needs **no API key**: `claude` carries its own OAuth
credential, so `bz --provider claude-code -m sonnet "q"` works on any machine where `claude` is
logged in.

Two new pieces of mechanism, both registry-shaped (arch §4):

- **An exec transport kind** (§3): a `WireRequest` may name a subprocess target instead of an
  HTTP one; the native transport spawns it, body → child stdin, child stdout → the body stream.
  One `Transport` seam, one new *mode* — never a vendor branch.
- **A `claude_code` protocol** (§4–§6): `ProtocolId::ClaudeCode`, mapping the canonical request
  onto the pinned `claude` argv + stdin, and claude's `--print` **stream-json** dialect (NDJSON)
  back onto canonical events. Its decode is a thin wrapper: the stream's `stream_event` payloads
  ARE Anthropic Messages SSE events, so it **delegates to the `anthropic_messages` decoder**
  (single source of truth; no second parser — the protocol-dedup rule).

Non-goals: driving any other CLI (the mechanism is general; only this dialect ships), tool
declarations through the CLI, multi-turn transcript replay (§4.2), and installing/updating
`claude` itself (the operator's job; a missing binary is a crisp in-band error, §3.4).

## 2. The pinned invocation (empirical ground truth)

`encode` produces exactly this child invocation — flags first, the prompt on **stdin** (no
argv-length limit, no quoting hazard; the POSIX filter idiom):

```
claude -p --output-format stream-json --include-partial-messages --verbose \
       --setting-sources "" --tools "" --disable-slash-commands --strict-mcp-config \
       --no-session-persistence --system-prompt <system-or-empty> --model <wire-model> \
       [--effort low|medium|high]
```

Each flag is load-bearing; all verified on v2.1.217:

| Flag | Why |
|---|---|
| `-p` | non-interactive print mode; never prompts, never dangles |
| `--output-format stream-json` | the NDJSON event dialect decode consumes |
| `--include-partial-messages` | streaming deltas (`stream_event` lines wrapping Messages SSE events) |
| `--verbose` | **required** by the CLI: `-p --output-format stream-json` refuses without it ("requires --verbose") |
| `--setting-sources ""` | load NO settings (user/project/local) → no hooks, no output styles, no CLAUDE.md auto-discovery (canary-verified: a cwd `CLAUDE.md` does not reach the model) |
| `--tools ""` | no built-in tools (`"tools":[]` in the init event) → nothing to loop on; one turn, no permission prompts |
| `--disable-slash-commands` | no skills |
| `--strict-mcp-config` | with no `--mcp-config`, zero MCP servers |
| `--no-session-persistence` | nothing written to disk, nothing resumable |
| `--system-prompt <s>` | **always passed**: replaces claude's default system prompt with the canonical `req.system` projection — `""` when the request has none (the empty set is not a special case) |
| `--model <m>` | the resolved wire model (`sonnet`/`opus`/`haiku`/full ids — claude's own vocabulary, passed verbatim) |
| `--effort <e>` | only when `req.reasoning` is set: the canonical knob's dialect spelling (§4.1) |

**`--bare` is rejected, deliberately.** `--bare` (and its env twin `CLAUDE_CODE_SIMPLE=1`) is the
CLI's own minimal mode — but it restricts Anthropic auth to `ANTHROPIC_API_KEY`/apiKeyHelper and
**never reads OAuth or the keychain** (verified: a logged-in machine gets "Not logged in · Please
run /login", exit 1). The CLI's own credential is this row's entire point, so suppression is
composed from the individual flags above instead.

**The owned residue (documented, not silent):** even with every source severed, claude injects
one `<system-reminder>` block (the account's `userEmail` and `currentDate`) into the first user
message — verified by asking the model to echo its input. It rides the OAuth-serving code path
and cannot be removed without `--bare` (which severs OAuth). ~150 input tokens; the model is
explicitly told it "may or may not be relevant". A pass-through consumer must not assume a
byte-clean context. The escape is the `anthropic` row with an API key.

`--max-turns` does not exist on this CLI version and is not needed: with zero tools there is
nothing to loop on (`num_turns: 1` observed on every capture).

## 3. The exec transport

### 3.1 The seam: `WireRequest.exec`, an `ExecSpec`

```rust
pub struct ExecSpec { pub program: String, pub args: Vec<String> }
// WireRequest gains:  pub exec: Option<ExecSpec>,   // None = HTTP (every existing dialect)
```

The one struct already crossing the transport seam carries the one new fact — exactly how
`method` and `timeouts` ride it (arch §4.1, model-discovery §6). `None` is the HTTP path,
byte-identical to before. When `Some`, the native transport spawns `program args…`, writes
`wire.body` to the child's stdin (then closes it), and returns the child's stdout as the
`TransportResponse.body` iterator. `url`/`method`/`headers` are inert on this path (stamped
harmlessly by the shared spine; the transport ignores them).

The protocol declares the target as data: `Protocol::exec_spec(&self, ctx) -> Option<ExecSpec>`
(default `None`) is the subprocess sibling of `path()` — the `--raw` spine, which skips `encode`,
stamps `wire.exec` from it exactly as it fills `wire.url` from `path()`. So `--raw` on an exec
row is coherent: raw-in feeds stdin bytes verbatim as the prompt; raw-out streams claude's NDJSON
verbatim. `encode` builds its own `ExecSpec` from the same one-home argv builder.

### 3.2 Status semantics: the spawn is the handshake

`TransportResponse.status` is **200** on every successful spawn. A subprocess has no HTTP
handshake; its failures reveal themselves **in the stream**, which is exactly the mid-stream
error model the pipeline already has (a 2xx stream with an in-band error, arch §3.3). So:

- **Spawn failure** (binary missing, not executable) → `Transport::send` returns `Err` — a
  `Transport`-kind `CanonicalError` naming the program and the OS error (exit 69), the exec
  analogue of an unreachable host.
- **CLI-reported failure** (logged out, bad model, API error) → decoded from the stream's
  `result` line into a crisp canonical error (§6). The CLI never dangles: `-p` mode fails
  instead of prompting (verified: logged-out exits 1 with a machine-readable stream).
- **Child died without a verdict** (crash, kill, flag error): stdout ends with no terminal
  marker → the existing premature-EOF `Transport` error (69). If the child exited nonzero
  **and** wrote stderr, the transport yields one trailing `Err` carrying the exit code and the
  stderr text, so the real diagnostic (e.g. a flag-parse error) survives instead of a bare EOF.
  A nonzero exit with empty stderr adds nothing (the stream verdict, when present, already told
  the truth — the logged-out capture exits 1 with an empty stderr).

**One round-trip, one POSIX exit** holds: one spawn per generation, `bz`'s own exit derived from
the in-band events exactly as on HTTP.

### 3.3 Timeouts & zombie prevention

The one resolved silence budget (`wire.timeouts`, config §4.3) applies as the **inter-chunk
stall bound on child stdout** — including time-to-first-byte — mirroring the HTTP transport's
`idle` bound: no bytes for `timeout` seconds → **kill the child**, reap it, and yield an `Err`
(→ `Transport`, 69). Total stream length is never capped (a long-but-live generation is never
cut). The child is always reaped: `wait()` on EOF, kill+wait on stall, and a kill+wait `Drop`
backstop on the body iterator (an abandoned stream leaves no zombie). Child stderr is drained
concurrently (never a pipe deadlock); stdin is written and closed from its own thread.

### 3.4 Placement

The spawn is impure, so it lives in the shim: `src/native/exec.rs`, routed from the wired
transport's `send` on `wire.exec.is_some()` — a mode branch on the request's declared shape,
like `method` (never a vendor name). `tests/purity.rs` grows `std::process::Command`/
`Command::new` in its forbidden set: the pure lib can no more spawn than it can open a socket.
`MockTransport` needs nothing new — it already records the whole `WireRequest`, `exec` included.

## 4. REQUEST mapping — canonical → argv + stdin

### 4.1 Field map

| Canonical | Wire | Note |
|---|---|---|
| `model` | `--model <m>` | already alias/cache-resolved (`ProviderCtx.model`); claude's aliases (`sonnet`, `opus`, `haiku`) are valid wire ids here |
| `system` | `--system-prompt <s>` | text blocks joined by `\n\n`; `None` → `""` (same flag, empty value — one path); a non-`Text` block **rejects** at encode (`ParseInput`/64, the arch §3.1 text-only-slot rule) |
| `messages` | stdin | exactly ONE `user` message of `Text` blocks, joined by `\n\n` (§4.2) |
| `reasoning` | `--effort low\|medium\|high` | the canonical knob's fifth dialect spelling (providers §6); `ReasoningEffort::as_str()` feeds it verbatim (verified accepted); `None` omits the flag |
| `stream` | *(fold only)* | the wire is ALWAYS stream-json — the dialect's one lossless shape; the resolved tri-state picks the fold, not the wire: `true` frames incrementally, `false` drains whole and replays through `decode_full` (§5.3). Honored, never reverted: both values produce the same bytes upstream and the same event vocabulary back |
| `max_tokens`, `temperature`, `top_p`, `stop`, `output` | — | **no wire slot** (the CLI exposes no such flags; `--json-schema` exists but is claude-side validation, not the API knob — not mapped). The shipped row lists all five in `unsupported_body_keys`, so they strip pre-encode (config §4.1.1) and the narrowing is visible row DATA; `encode` has no slot to read regardless, so a custom row that omits the strip changes nothing |
| `tools`, `tool_choice != Auto` | — | **reject** at encode (`ParseInput`/64): the CLI cannot carry caller tool declarations, and a strip would silently change semantics. `tools: []` + `Auto` is the normal no-tools path |
| `parallel_tool_calls` | — | inert without tools; omitted |
| `extra` | — | **dropped** at encode, documented: the wire has no JSON body to fold the long-tail valve into. The one dialect where arch §3.1's forward-the-unknown contract cannot reach the provider — the owned inverse, stated here so it is a decision, not an oversight |

### 4.2 Single-turn only (a documented narrowing)

`messages` must be exactly one `user` message of `Text` content. Anything else — assistant
history, tool blocks, images, documents, a `Role::System` positional message — **rejects at
encode** with `ParseInput`/64 naming the offender. The CLI's print mode takes one prompt; it
cannot replay an assistant turn, so any projection of a transcript would be fabrication, not
translation. Multi-turn callers use the `anthropic` row. (This is the arch §3.1 reject-at-encode
rule, not a canonical-model change.)

### 4.3 Encode output

`encode` yields `WireRequest { url: "", body: <prompt bytes>, exec: Some(spec), .. }` with
`spec.program = ctx.exec` (§7) and `spec.args` = the §2 flag list. `path()` is `""` and
`content_type()` is `text/plain` (stdin is prose, not JSON); both are inert on the exec path but
keep the shared spine oblivious. `ctx.exec` absent (a hand-written `claude_code` row with no
`exec`) → a `Config`-kind encode error (78) naming the missing field.

## 5. RESPONSE mapping — stream-json NDJSON → canonical events

### 5.1 Framing

`Framing::Ndjson` — claude's stream-json is newline-delimited JSON, consumed by the existing
NDJSON framer verbatim (sse-decoder §4). One line = one frame.

### 5.2 Line dispatch (the whole decoder)

Dispatch on the line's `type`:

| Line | → |
|---|---|
| `stream_event` | unwrap `.event` — it IS an Anthropic Messages SSE payload (`message_start`, `content_block_start/delta/stop`, `message_delta`, `message_stop`, verbatim, signatures and all) — and **delegate to the `anthropic_messages` decoder** with the same `DecodeState`. Text/thinking/signature deltas, usage, `Finish`, and the `message_stop` terminator all fall out of the one existing state machine; nothing is parsed twice |
| `assistant` | aggregate duplicate of the stream on success → nothing; on failure it carries the classifying tag (`"error": "authentication_failed"`) → noted into `DecodeState.error_tag` for the `result` fold (§6) — the fact carried, never re-derived from message strings |
| `result` | the CLI's own terminator: sets `terminated` (so EOF after it is clean). `is_error: false` after a completed message → nothing (the inner `message_stop` already terminated); `is_error: false` with NO message stream → a malformed run: in-band `Transport` error ("no completion"); `is_error: true` → the canonical error (§6) |
| `system`, `rate_limit_event`, unknown | nothing — envelope chatter (the arch §3.2 unknown-provider-block drop) |

A whole-body error frame (`frame.status: Some`) delegates to the same shared `http_error` fold
every dialect uses — unreachable from the shipped exec transport (status is always 200, §3.2)
but reachable through the seam by any embedder's transport, so it stays uniform and tested.

### 5.3 `decode_full`

The non-stream fold splits the drained body on newlines and replays each line through `decode`
— the explode→replay rule (arch §3.2) in its degenerate form, since the aggregate body IS the
stream's own lines. `run`'s `ensure_terminal` guard then covers a verdict-less body.

### 5.4 What the stream carries

From the real capture: `MessageStart` (id/model from the inner `message_start`), `Usage`
(cumulative, from `message_start` and the terminal `message_delta`), thinking blocks with
`ThinkingDelta` + `SignatureDelta` (haiku thinks unprompted; signatures round-trip), text
blocks, `Finish{Stop}` from `stop_reason: "end_turn"`, and the one `End` appended by `run`.
Identical vocabulary, zero new events.

## 6. ERROR mapping

On a `result` line with `is_error: true`, the error kind is derived from carried facts, in
order:

1. `api_error_status` (a number = the upstream API's HTTP status, carried by the CLI) →
   `ErrorKind::from_http_status(status)` — the one shared status table (429→69/retryable,
   5xx→70, 401/403→77…).
2. else `DecodeState.error_tag == "authentication_failed"` (from the `assistant` line, §5.2) →
   `ErrorKind::Auth` (77). The logged-out capture takes this arm: exit 77, message
   `Not logged in · Please run /login`.
3. else `ErrorKind::Transport` (69) — the response-side safe default every decoder uses.

`message` = the `result` string verbatim; `provider_detail` = the whole `result` object
verbatim. `retry_after_seconds` stays `None` (no HTTP handshake to carry the header — the
empty-set rule, never fabricated).

## 7. Config: the row, the `exec` field, resolution

### 7.1 The shipped row (`data/defaults.toml`)

```toml
[[provider]]
name = "claude-code"
protocol = "claude_code"
auth = "none"
exec = "claude"
unsupported_body_keys = ["max_tokens", "temperature", "top_p", "stop", "output"]
```

- **`exec` is a new optional row field**: the subprocess program (a name resolved on `PATH`, or
  an absolute path). It substitutes for `base_url`: a row carrying `exec` may omit `base_url`
  (resolution completes it as `""`); a row with neither still fails `IncompleteProvider`
  (base_url). No `ProtocolId` pairing invariant: `exec` on an HTTP-dialect row is simply unread
  (like `ambient`), and a `claude_code` row without `exec` fails at encode (§4.3).
- **`auth = "none"`** — `NoAuth` reads no credential and writes no header; `claude` brings its
  own OAuth. `bz --login --provider claude-code` is meaningless and keeps failing resolution
  the standard keyless way; log in with `claude` itself.
- **No `model_prefixes`**: the first-declared `anthropic` row owns `claude-`; this
  alternate-transport row stays opt-in via explicit `--provider`, exactly like
  `openai-responses` (arch §4.3.1 — a shipped default must not encode an accidental priority
  between two rows serving one family).

### 7.2 Model discovery: an honest decline

`Protocol::models_shape` becomes **`Option<ModelsShape>`** — the same decline shape as
`count_tokens` (arch §5.10.1): `None` = this dialect has no models listing, and
`bz --list-models` fails with a `Config` error (78) telling the caller to pass `--model`
verbatim. The five HTTP dialects return `Some` unchanged. `claude_code` declines: the CLI has
no list command, and inventing a static list would be a second home for Anthropic's catalogue.
A row's `[provider.models]` override cannot conjure a listing over a `None` base (there is no
endpoint to point it at); the decline wins.

The cache still works forward: **learn-on-success** (model-discovery §5.4) appends each model
the CLI accepts, so the first call names a model (`bz --provider claude-code -m sonnet "q"`)
and every later bare call rides `last_used`. A cold cache with no model remains the standard
`select_model` 78. Owned cost: the 200-at-spawn status (§3.2) means a run whose *stream* failed
still learns its model — a bad id can enter the cache; it falls out on the next explicit `-m`.

### 7.3 Severability

Delete the row → the capability is config-gone. Delete `protocol/claude_code/` + the
`ProtocolId` arm → compile-enforced removal, rows referencing it fail resolve (arch §4.6).
Delete `native/exec.rs` + the `wire.exec` field → the transport kind is gone and every HTTP
dialect is untouched. No flag, no verb, no new canonical surface.

## 8. Testability

- **Golden fixtures, real captures, committed verbatim** (arch §9.2):
  `tests/fixtures/claude_code_basic.ndjson` (the §2 invocation against v2.1.217: thinking +
  signature + text + usage + result) and `claude_code_error_loggedout.ndjson` (the logged-out
  run: init/status/assistant-with-error-tag/result-is_error). Both run through the adversarial
  rechunker (arch §9.3) — decode is chunk-boundary-blind.
- **Encode**: pure table tests — the pinned argv (flag order and all), stdin bytes, system
  join, `--effort`, each reject arm (multi-message, non-text, tools, non-Auto choice, missing
  `exec`).
- **Decode**: fixture-driven plus targeted lines for each `result` arm (`api_error_status`
  number, auth tag, bare error, success-without-message) and the delegated status frame.
- **Pipeline**: `run` end-to-end with `MockTransport` asserting the captured `WireRequest`
  (argv + stdin body + `exec`), the `--raw` exec stamp, the `--list-models` decline, and the
  resolve rules (`exec` fold, base_url-optional-with-exec, dump-config round-trip).
- The **live** proof is manual like every live path: `bz --provider claude-code -m sonnet
  "say pong"` on a logged-in machine (recorded in the delivering task).

## 9. Summary of decisions

1. Suppression is **composed flags, not `--bare`** — `--bare` severs the CLI's own OAuth, which
   is the row's reason to exist. The userEmail/currentDate system-reminder is the owned,
   documented residue.
2. Prompt on **stdin**; system via `--system-prompt` (always passed, `""` for none).
3. **Single-turn, text-only** dialect: one user message; everything else rejects at encode
   (`ParseInput`/64). No transcript fabrication.
4. `reasoning` → `--effort` (the knob's fifth spelling); `max_tokens`/`temperature`/`top_p`/
   `stop`/`output` have no wire slot — stripped by row data, documented. `extra` is dropped —
   the one dialect where the forward valve cannot reach the provider.
5. The wire always streams (stream-json); the `stream` tri-state picks the fold
   (`decode`/`decode_full`), not the bytes.
6. Transport: `WireRequest.exec: Option<ExecSpec>` + `Protocol::exec_spec` (data, like
   `path()`); native spawn in `src/native/exec.rs`; status 200 at spawn; failures in-band;
   stderr carried on nonzero exit; silence budget kills the child; always reaped.
7. Decode **delegates** `stream_event` payloads to the `anthropic_messages` decoder — one
   Messages parser in the codebase. `result` is the dialect's own terminator and error
   envelope; `DecodeState.error_tag` carries the auth classification fact.
8. `models_shape` → `Option`: `claude-code` **declines** `--list-models` (78, with the next
   move in the message); the cache fills by learn-on-success.
9. The row ships in `data/defaults.toml`, keyless, claiming no model family (explicit
   `--provider`, like `openai-responses`).
