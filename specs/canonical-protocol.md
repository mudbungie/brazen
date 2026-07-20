# Canonical protocol: the `bz --json` consumer contract

> **Living document.** Edited like code. This spec derives from the canonical contract in
> architecture.md and MUST NOT contradict it — architecture.md remains the single source of
> truth; where this document restates for readability, each section names the section it
> derives from, so drift is detectable. Where it cannot be written without changing the
> architecture, it raises a change request to architecture.md rather than silently deviating.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — §3.1, §3.2, §3.3, §3.4, §5, §8.

---

## 1. Purpose & audience

This is the protocol spec for anyone building **on** brazen — piping a canonical JSON
request into `bz` and consuming the NDJSON event stream — who will never read brazen's
source or its internal architecture. The primary consumer is an agent-harness layer; the
same contract serves any `--json` scripter.

The contract in one paragraph: you write **one JSON document** (the canonical request, §2)
to `bz`'s stdin (or `--input FILE`; a positional prompt is a shorthand constructor for the
same request), you invoke with `--json`, and you read **newline-delimited JSON events**
(§3) from stdout until the literal line `{"type":"end"}`, then EOF. The process exit code
(§5) is a coarse POSIX failure class; everything finer is in the events. brazen is
stateless and single-shot: it holds no conversation, never retries, and executes no tools —
multi-turn state, retry loops, and tool execution are yours, which is why the replay
obligations of §2.5 exist.

What you never need to know: which provider dialect is on the wire. The request and event
vocabularies here are provider-agnostic; brazen owns every projection.

## 2. The canonical request

*(derives from architecture.md §3.1)*

One JSON object. Every field is optional at the parse layer; a field you set is used
as-is, a field you omit is filled from brazen's config fold (flags > env > config file >
provider-row defaults) — the pipe is clean data, not a config layer. A malformed document
is an in-band `parse_input` error and exit 64.

### 2.1 Top-level fields

| Field | Type | Omitted means | Notes |
|---|---|---|---|
| `model` | string | config supplies it | canonical alias or wire id; routes to the owning provider row |
| `system` | array of content (§2.2) | no system prompt | the **leading** system prompt. Must be an array (elements may be bare strings); a bare string here is a parse error. A `system`-role *message* is a different fact: a system turn at a specific transcript position |
| `messages` | array of `{role, content}` | empty transcript | the conversation; see §2.2 |
| `tools` | array of tool objects (§2.3) | no tools | empty array = no tools; never null |
| `tool_choice` | `{"type":"auto"\|"any"\|"none"}` or `{"type":"tool","name":…}` | `auto` | lifted knob #1 |
| `parallel_tool_calls` | bool | provider default | lifted knob #2 |
| `reasoning` | `"low"` \| `"medium"` \| `"high"` | no reasoning requested | lifted knob #3, portable effort intent; brazen maps it to each dialect's native shape (budget tokens, effort string, …) |
| `output` | `{"type":"json"}` or `{"type":"json_schema","schema":…,"name"?,"strict"?}` | plain text | lifted knob #4, structured output. `name`/`strict` feed only the dialects that have them; Anthropic lacks schemaless `json` (documented narrowing) |
| `max_tokens` | integer | provider-row default | |
| `temperature`, `top_p` | number | provider default | |
| `stop` | array of strings | no stop sequences | |
| `stream` | bool | resolved by config; global default **true** | tri-state, honored end to end: `false` makes brazen fetch one aggregate body — the **output events are identical either way** (§4) |
| *anything else* | any | — | forwarded verbatim: the `extra` valve, §2.4 |

### 2.2 Messages and content blocks

A message is `{"role": "system"|"user"|"assistant"|"tool", "content": …}`. `role: "tool"`
is the canonical home for tool results even though some providers spell it differently —
brazen owns that projection.

`content` (and `tool_result.content`) accepts **a bare string, a single content object, or
an array of content** — all decode to the same array; `"hi"` ≡ `[{"type":"text","text":"hi"}]`.
Inside an array, a bare string element is likewise a text block.

The content block vocabulary (`type`-tagged objects):

| `type` | Fields | Direction |
|---|---|---|
| `text` | `text` | in + out |
| `image` | `source`: `{"kind":"base64","media_type":…,"data":…}` or `{"kind":"url","url":…}` | input only |
| `document` | `source`: same two-variant shape as `image` (PDFs/files) | input only |
| `tool_use` | `id`, `name`, `input` (object), `signature`? | replay of an assistant tool call |
| `tool_result` | `tool_use_id`, `content` (string/object/array), `is_error` (default false) | your tool's answer, in a `tool`-role message |
| `thinking` | `text`, `signature`?, `id`?, `encrypted_content`? | replay of a reasoning block |
| `redacted_thinking` | `data` | replay, opaque (§2.5) |
| `server_tool_use` | `id`, `name`, `input` | replay, opaque (§2.5) |
| `*_tool_result` (open set: `web_search_tool_result`, …) | `tool_use_id`, `content` (verbatim payload) | replay, opaque — any `type` ending `_tool_result` other than the client `tool_result` itself; the tag round-trips as data |

Media narrowings are **loud, never silent**: a dialect whose wire cannot express a block or
a source variant rejects at encode with a `parse_input` error, exit 64 (e.g. OpenAI Chat
and Google reject `document` + `url`; Ollama has no document slot at all). The per-dialect
table is providers.md §9.

### 2.3 Tools and the four lifted knobs

A tool object's shape declares its class by the **presence of a `type` key**:

- **No `type` key → a custom (caller-defined) tool** brazen normalizes across dialects:
  `{"name":…, "description"?, "input_schema": <JSON Schema>, "strict"?}`. `strict` is the
  OpenAI-style strict-function-calling toggle (structured-output's per-tool sibling);
  dialects without it narrow (documented, providers.md §6).
- **A `type` key → a provider-typed tool** carried **verbatim** to the routed provider
  (`{"type":"web_search_20250305","name":…, …config}`): brazen has no opinion on the
  `type`; a bad one is the provider's 400. This covers provider client tools *and* server
  tools.

Four request intents are **typed fields, not `extra` keys**, because every dialect spells
them differently and a passthrough could only speak one spelling: `tool_choice`,
`parallel_tool_calls`, `reasoning`, `output` (§2.1). Prefer them over any provider-native
spelling — the typed knob **wins** over a same-named provider key arriving via `extra` or
row `body_defaults`, so the two never silently combine. A provider row that cannot accept
one lists the canonical key in its `unsupported_body_keys` and brazen strips it pre-encode.

### 2.4 The `extra` valve — and what a typo costs you

Any top-level request key brazen does not model is **forwarded to the provider verbatim**
(`safetySettings`, a provider-shaped adaptive-thinking object, …). This is the long-tail
escape valve, and its cost is deliberate and owned: brazen does **not** validate the long
tail, so a **misspelled canonical field** (`temperatue`, `mesages`) silently becomes a
passthrough key and surfaces — if at all — as the provider's 4xx (exit 69), never as a
local exit-64 parse error. Spell the §2.1 fields exactly.

### 2.5 Multi-turn replay obligations

brazen is stateless; **you** carry the transcript. When you rebuild prior assistant turns
from the event stream (the fold recipe is §3.3) and re-send them in `messages`, several
provider payloads are **load-bearing opaque blobs** the upstream API rejects (400) if
altered, reordered, or dropped. Round-trip each **verbatim**:

- `thinking.signature` — Anthropic's thinking signature. Never modify, never fabricate.
- `redacted_thinking.data` — Anthropic, encrypted. Keep the block, keep its position.
- `tool_use.signature` — Google's `thoughtSignature`, attached to the tool call it came
  with; Gemini multi-turn function calling 400s without it.
- `thinking.id` and `thinking.encrypted_content` — OpenAI Responses reasoning-item replay
  (`encrypted_content` appears when the request asked for it via
  `include:["reasoning.encrypted_content"]`, an `extra` key).
- `server_tool_use` / `*_tool_result` blocks — replay verbatim, in place. Never convert a
  `server_tool_use` into a `tool_use` and never answer it with a client `tool_result`: the
  provider already executed it, and a reshaped replay 400s.

Every one of these fields is optional and absent (`null`/omitted) on dialects without the
concept — absence is fine; alteration is not. The general rule: **anything opaque you
received, return untouched**.

### 2.6 A complete request

```json
{
  "model": "claude-sonnet-4-5",
  "system": ["You are terse."],
  "messages": [
    {"role": "user", "content": "What is the weather in Paris?"}
  ],
  "tools": [
    {"name": "get_weather", "description": "Current weather for a city",
     "input_schema": {"type": "object", "properties": {"city": {"type": "string"}},
                      "required": ["city"]}}
  ],
  "tool_choice": {"type": "auto"},
  "reasoning": "low",
  "max_tokens": 1024
}
```

## 3. The event stream

*(derives from architecture.md §3.2, §5.2, §5.6)*

### 3.1 Framing

Under `--json`, stdout is **NDJSON**: one event object per line, `\n`-terminated, UTF-8,
flushed after every line (embedded newlines are JSON-escaped, so a line break is always a
frame boundary). Read lines until `{"type":"end"}`, then expect EOF. **Every stream ends
with exactly one `end` line, even on failure** — there is no "did it finish?" edge case.
Errors are in-band events on stdout; stderr is silent on this path.

### 3.2 The vocabulary

Every event is `type`-tagged. The full set, each with its wire shape:

| Event | Worked example line |
|---|---|
| `message_start` | `{"type":"message_start","v":1,"id":"msg_01…","model":"claude-sonnet-4-5","role":"assistant"}` |
| `content_start` | `{"type":"content_start","index":0,"kind":{"text":{}}}` |
| `content_delta` | `{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}` |
| `content_stop` | `{"type":"content_stop","index":0}` |
| `usage` | `{"type":"usage","input_tokens":36,"output_tokens":10,"cache_read_tokens":null,"cache_write_tokens":null}` |
| `finish` | `{"type":"finish","reason":"stop"}` |
| `error` | `{"type":"error","kind":{"provider":{"status":429}},"message":"…","provider_detail":{…}}` |
| `end` | `{"type":"end"}` |

- **`message_start`** opens every successful stream, first. `v` is the event-schema
  version handshake (§3.6). `id`/`model` are `null` when the provider doesn't supply them.
- **`content_start` / `content_delta` / `content_stop`** frame each content block by
  `index`. Block **identity always precedes content**: the `content_start` carries the
  block's kind and ids before any delta, on every provider — you never need a "did I see
  the id yet?" branch. The `kind` values (externally tagged, `{tag: body}`):

  | `kind` | Body at block open | Deltas that follow |
  |---|---|---|
  | `text` | `{}` | `text_delta` |
  | `tool_use` | `{"id":…,"name":…}` | `json_delta`, `signature_delta`? |
  | `thinking` | `{}` or `{"id":"rs_…"}` (OpenAI Responses item id) | `thinking_delta`, `signature_delta`?, `encrypted_reasoning_delta`? |
  | `redacted_thinking` | `{"data":"<opaque>"}` — full payload inline at open | none |
  | `server_tool_use` | `{"id":…,"name":…}` | `json_delta` |
  | `<wire tag>` (server-tool result, open set) | `{"kind":{"web_search_tool_result":{…}}}` — full content inline | none |

- **Delta variants** (externally tagged): `text_delta`, `thinking_delta` (strings to
  concatenate); `json_delta` — tool-call **argument text fragments**, valid JSON only when
  concatenated: assemble the whole string, then `JSON.parse` it — never string-match a
  fragment; `signature_delta` — the opaque signature for the block at this index (fold onto
  the open block's `signature` — a thinking block's Anthropic signature or a tool block's
  Google `thoughtSignature`, one grain for both), arriving just before the block's stop;
  `encrypted_reasoning_delta` — OpenAI Responses `encrypted_content`, fold onto the open
  thinking block, also just before its stop.
- **`usage`** counters are **cumulative** and every field is nullable: `null` means
  *unknown*, never zero — a provider that doesn't report a counter leaves it `null`, and
  brazen never fabricates a `0`. Emitted whenever the provider reveals usage (possibly more
  than once; the last one is the total).
- **`finish`** says **why generation stopped**; its `reason` values: `"stop"`, `"length"`,
  `"tool_use"`, `"stop_sequence"`, `"pause"`, `"refusal"` (with flat sibling keys
  `category` and `explanation`:
  `{"type":"finish","reason":"refusal","category":"…","explanation":null}`), or any unknown
  string passed through verbatim. **A refusal is a `finish`, never an `error`** — it
  arrives as HTTP 200 and exits 0.
- **`error`** — §3.5. `finish` and `error` are two distinct truths: a response either
  finished (with some reason, possibly refusal) or it errored; the two are never folded.
- **`end`** means **the byte stream is over** — emitted exactly once, last, after any
  `finish`/`usage`/`error`. `finish` ≠ `end`.

### 3.3 Stream guarantees, and the fold back to content

Guarantees you may build on:

1. **One `end`, always last, even on failure** (§3.1).
2. **`message_start` first** on every stream that gets far enough to have a message; a
   stream may instead open with `error` (then `end`) when the failure precedes any message.
3. **Identity before content**: every block's `content_start` precedes its deltas.
4. **Every `content_start` is eventually followed by its `content_stop`** — on clean
   streams and on failure alike (on a mid-stream failure brazen closes all open blocks
   *before* the injected `error`). Per-block state never leaks.
5. **A terminal verdict exists**: a completed stream carries a `finish` or an `error`
   (or both — partial response then mid-stream failure is representable as
   `… content_stop, error, end`).
6. **A premature upstream disconnect** is an in-band `error` with kind `transport`
   (exit 69), never a silently truncated success.

The fold — how a harness rebuilds the assistant turn for replay (§2.5): for each `index`,
open a block at `content_start`, apply deltas, close at `content_stop`:

| Stream | → request content block |
|---|---|
| `text` + `text_delta`* | `{"type":"text","text":<concat>}` |
| `tool_use{id,name}` + `json_delta`* (+ `signature_delta`) | `{"type":"tool_use","id":…,"name":…,"input":JSON.parse(<concat>),"signature"?}` |
| `thinking{id?}` + `thinking_delta`* (+ `signature_delta`, `encrypted_reasoning_delta`) | `{"type":"thinking","text":<concat>,"signature"?,"id"?,"encrypted_content"?}` |
| `redacted_thinking{data}` | `{"type":"redacted_thinking","data":…}` — verbatim |
| `server_tool_use{id,name}` + `json_delta`* | `{"type":"server_tool_use","id":…,"name":…,"input":JSON.parse(<concat>)}` — verbatim |
| server-tool result kind | `{"type":"<wire tag>","tool_use_id":…,"content":…}` — verbatim |

Blocks in `index` order form the assistant message's `content`; append your
`tool`-role message with a `tool_result` per `tool_use` id, and send the grown `messages`
back (worked in §6.2).

### 3.4 What the other output modes change

`--json` is the full contract above. The projections drop data, never reshape it:
`--text` (default) emits only the concatenated `text_delta` bytes (errors go to stderr;
terminator is stdout EOF, no `end` line); `--thinking` adds the reasoning text before the
answer. `--raw` streams provider-native bytes — none of this spec applies there except the
exit codes. A harness wants `--json`.

### 3.5 The error event

*(derives from architecture.md §3.3)*

```json
{"type":"error","kind":{"provider":{"status":404}},"message":"model 'x' not found; …",
 "provider_detail":{"error":"model 'x' not found"},"retry_after_seconds":30}
```

- **`kind`** is the failure taxonomy: `"usage"`, `"parse_input"`, `"config"`, `"auth"`,
  `"transport"`, `"interrupted"`, or `{"provider":{"status":<HTTP status>}}`. An
  unrecognized future tag must be treated as opaque (§3.6).
- **`message`** — one human-readable line.
- **`provider_detail`** — the upstream error body, parsed, verbatim (`null` when there is
  none). Hints an exit code can't carry live here.
- **`retry_after_seconds`** — the provider's `Retry-After` **header** in whole seconds,
  present only when a non-2xx handshake carried one; the pacing hint for your retry loop.
  Omitted when absent — never fabricated.
- **`retryable` is computed, not a field**: retry when `kind` is `"transport"`, or
  `provider` with `status == 429 || status >= 500`. brazen itself never retries.

Errors are **in-band**: a stream may deliver half a response and then an `error`, and an
`error` may follow `finish`-less content. The stream still ends with `end`.

### 3.6 Versioning & stability: `v` and the additive-change policy

*(derives from architecture.md §3.2, the `v=1` contract)*

`message_start.v` — currently **1**, from brazen's single `EVENT_SCHEMA_VERSION` — is the
one handshake to pin. An error-first stream has no `message_start` and so no `v`; a
consumer that gets `error` first needs no version to act.

**Within a `v`, the vocabulary only grows.** Your obligations as a pinned consumer:

- **Ignore** an unknown event `type`, an unknown content `kind`, an unknown `delta`
  variant, an unknown `finish.reason` string, an unknown `error.kind` tag, and unknown
  object fields anywhere — skip or pass through, never error. (Unknown kinds/deltas arrive
  as their verbatim `{tag: body}` object, so passthrough is possible.)
- In return, brazen guarantees **additions never bump `v`**: a new event, kind, delta,
  field, or usage counter is additive. `v` bumps **only** for a removal, a rename, or a
  semantic change to an existing value — which a pinned consumer may refuse.
- The error schema has no `v` gate and is governed by the same grows-only rule.

The exit-code table (§5) and the NDJSON framing (§3.1) are frozen alongside the `v=1`
vocabulary.

## 4. Stream vs aggregate

*(derives from architecture.md §3.2, §5.2)*

Output is a stream **even when the wire is not**. With `"stream": false` (or
`--no-stream`), brazen fetches one aggregate JSON body and replays it through the same
vocabulary: you read the identical NDJSON event sequence — typically one big
`content_delta` per block instead of many small ones (§6 shows a capture). No second
shape to parse. A 200 aggregate that carries neither a `finish` nor an `error` is
malformed and surfaces as an in-band `transport` error, exit 69 — never a silently-empty
success.

## 5. Exit codes

*(derives from architecture.md §8 — frozen, deliberately coarse)*

| Code | Meaning |
|-----:|---|
| 0 | success — **including refusal** (`finish.reason == "refusal"`) |
| 64 | usage / bad input: unknown flag, malformed request JSON, unrepresentable content at encode |
| 66 | `--input`/`-f` file missing or unreadable |
| 69 | transport (connect/DNS/TLS/timeout/premature EOF) or upstream HTTP **4xx** (incl. 429) |
| 70 | upstream HTTP **5xx** |
| 77 | auth: 401/403, missing credentials, refresh failure |
| 78 | config: no provider resolved, unknown provider/model, contradictory config |
| 130 / 141 / 143 | SIGINT / SIGPIPE / SIGTERM |

Do not branch retry policy on the exit code — 69 mixes retryable (transport, 429) with
non-retryable (400). The structured channel is strictly finer: read
`error.kind.provider.status`, the computed-retryable rule (§3.5), and
`retry_after_seconds`. The exit code is for coarse scripting; the events are the protocol.

## 6. Worked examples

Real captures (Ollama row, `--json`; byte shapes identical across providers — that is the
point).

### 6.1 A text turn

Request in:

```json
{"messages":[{"role":"user","content":"Reply with exactly the word hello."}],"max_tokens":16}
```

Event stream out:

```
{"type":"message_start","v":1,"id":null,"model":"bz-smoke:latest","role":"assistant"}
{"type":"content_start","index":0,"kind":{"text":{}}}
{"type":"content_delta","index":0,"delta":{"text_delta":"Hello"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"!"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" How"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" can"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" I"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" assist"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" you"}}
{"type":"content_delta","index":0,"delta":{"text_delta":" today"}}
{"type":"content_delta","index":0,"delta":{"text_delta":"?"}}
{"type":"content_stop","index":0}
{"type":"usage","input_tokens":36,"output_tokens":10,"cache_read_tokens":null,"cache_write_tokens":null}
{"type":"finish","reason":"stop"}
{"type":"end"}
```

Exit 0. The same request with `"stream": false` yields the same sequence with one
`content_delta` carrying the whole text (§4).

### 6.2 A tool-call turn, and the replay that follows

Request in:

```json
{"messages":[{"role":"user","content":"What is the weather in Paris? Use the get_weather tool."}],
 "tools":[{"name":"get_weather","description":"Get current weather for a city",
           "input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}],
 "max_tokens":64}
```

Event stream out:

```
{"type":"message_start","v":1,"id":null,"model":"bz-smoke:latest","role":"assistant"}
{"type":"content_start","index":0,"kind":{"tool_use":{"id":"call_0","name":"get_weather"}}}
{"type":"content_delta","index":0,"delta":{"json_delta":"{\"city\":\"Paris\"}"}}
{"type":"content_stop","index":0}
{"type":"usage","input_tokens":177,"output_tokens":20,"cache_read_tokens":null,"cache_write_tokens":null}
{"type":"finish","reason":"tool_use"}
{"type":"end"}
```

Exit 0. `finish.reason == "tool_use"` is your cue: concatenate the block's `json_delta`
fragments (here one), `JSON.parse` them, run the tool yourself, then send the next turn —
the §3.3 fold of the assistant turn, plus your `tool`-role result:

```json
{"messages":[
   {"role":"user","content":"What is the weather in Paris? Use the get_weather tool."},
   {"role":"assistant","content":[
      {"type":"tool_use","id":"call_0","name":"get_weather","input":{"city":"Paris"}}]},
   {"role":"tool","content":[
      {"type":"tool_result","tool_use_id":"call_0","content":"18°C, overcast","is_error":false}]}
 ],
 "tools":[{"name":"get_weather","description":"Get current weather for a city",
           "input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}],
 "max_tokens":64}
```

(Keep declaring `tools` on every turn. Had the turn carried thinking blocks or signatures,
they replay verbatim inside that assistant message — §2.5.) The second stream is then an
ordinary text turn, as in §6.1.

### 6.3 An error stream

Request routed to a model the provider doesn't have:

```
{"type":"error","kind":{"provider":{"status":404}},"message":"model 'nonexistent-model' not found; `nonexistent-model` is not in the model cache; run `bz --list-models` to refresh or enable partial matching","provider_detail":{"error":"model 'nonexistent-model' not found"}}
{"type":"end"}
```

Exit 69. No `message_start` (nothing began), no `v` — but still the one `end`.
