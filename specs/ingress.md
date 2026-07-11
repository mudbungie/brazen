# Ingress: the masquerade surface (M-in / N-out)

> **Living document.** Edited like code. This spec derives from the canonical contract in
> architecture.md and MUST NOT contradict it. The architecture.md §1/§2 amendments this
> capability requires are enumerated in §13 and land in the same change as this spec —
> architecture.md changes first, per the specs/README.md convention.
> **Derives from:** [Architecture & I/O Contract](architecture.md) — §2, §3, §4.3, §5, §8;
> [Config](config.md) — §2, §3, §7; [OpenAI chat mapping](openai-chat-mapping.md).

---

## 1. Purpose & Scope

brazen is a universal adapter — but until now only on the egress side: canonical in, any
provider out. **Ingress** is the mirror: brazen accepts a request in a *client dialect*
(OpenAI `chat/completions` first), decodes it to the canonical model, runs the ordinary
pipeline against whatever upstream the config routes to, and re-encodes the canonical event
stream back into the client's dialect. A harness that only speaks OpenAI can drive Anthropic
— or anything brazen speaks — by pointing its `base_url` at brazen.

**The payoff of the canonical hub is that this costs M ingress codecs, never M×N bridges.**
Both edges meet at the canonical model; an OpenAI-speaking client reaching Google works the
moment `openai_chat` ingress exists, with zero pair-specific code. The canonical model is
unchanged by this spec — every concern here lives at the new edge, not in the hub.

**In scope:** the ingress codec contract (§2); the adapt-or-reject ladder and the
spec-not-policy boundary (§3); the lossy-adaptation knob and its runtime exposure (§4); the
replay stash (§5); routing and the `[ingress]` config table (§6); the `--serve` listener
(§7); pseudo-routes (§8); error masquerade (§9); response-shape streaming (§10); the
one-shot `--in` filter (§11); wave scoping (§12); the architecture.md amendments (§13); the
test story (§14).

**Out of scope:** new canonical fields (none are needed — bl-61a9 already gave every opaque
replay payload a canonical home); TLS on the listener (localhost is the deployment; a
reverse proxy owns TLS if anyone wants it); the Anthropic/Google/Responses ingress dialect
*contents* (wave 2+, §12 — they reuse this spec's machinery, each adding only its codec
pair).

---

## 2. The shape: one codec pair per ingress dialect

An ingress dialect is exactly two pure functions, the mirror of the egress adapter's
`encode`/`decode`:

```rust
/// Client-dialect request bytes -> the canonical request.
/// Errors are ErrorKind::ParseInput framed in the CLIENT dialect (§9).
fn decode_request(bytes: &[u8]) -> Result<CanonicalRequest, IngressError>

/// The canonical event stream -> client-dialect response bytes.
/// Total: consumes every Event including Error (§9); shape per the client's
/// stream knob (§10). Streaming-capable: called per event, emits zero or
/// more byte chunks (SSE frames or the growing aggregate).
fn encode_response(event: &Event, state: &mut IngressState) -> Vec<u8>
```

Rules inherited from the egress side, unchanged:

- **Vendor-blind core.** The pipeline never learns which ingress dialect fed it; the
  dialect is resolved at the edge (config/flag, §6/§11) and dispatched through the same
  registry pattern as egress protocols.
- **No sniffing, ever.** The ingress dialect is always named explicitly — by the
  `[ingress]` config table (§6) or the `--in` flag (§11). architecture.md §2's amended
  non-goal (§13) forbids structural sniffing exactly as before; what it no longer forbids
  is *explicit* non-canonical input.
- **Lossy projections, honestly.** `decode_request` maps known dialect fields onto the
  canonical request (`response_format` → `output`, `reasoning_effort` → `reasoning`,
  `tool_choice` → `ToolChoice`, …). Unknown top-level keys ride the canonical `extra`
  valve verbatim — the same forwarded-not-rejected stance as canonical input
  (architecture.md §3.1), and the same misspelling cost, owned.
- **Fidelity is a maintained target.** The encoder must produce what real SDKs parse:
  exact SSE chunk shapes (index-carrying tool-call deltas, `id` on the first chunk,
  `[DONE]` sentinel, `usage` on the final chunk when `stream_options.include_usage`
  asks), fabricated-but-well-formed identity fields (`id`, `created` from the injected
  `Clock`, `object`), and the dialect's `finish_reason` vocabulary mapped from canonical
  `Finish`. Upstream dialect drift is already a treadmill brazen runs on egress; ingress
  doubles the belt, not the kind of work (§14 makes drift loud).

---

## 3. The adapt-or-reject ladder

**A predictable upstream 400 is a brazen bug.** brazen is an adapter, not a frontend:
merely propagating a failure it could have prevented — or could have adapted around — is a
product failure. For every client-request feature meeting an upstream capability, exactly
one rung applies, evaluated in order:

1. **Representable → transform, silently.** The upstream wire has a slot for the intent;
   this is just encoding (effort→budget, system message→`system` param). The client has no
   stake in the spelling.
2. **Zero-loss adaptation exists → adapt, silently.** The wire lacks the client's shape but
   an equivalent exists with no meaning lost. Zero-loss decisions are made *for* the
   client.
3. **Only a lossy adaptation exists → adapt by default, knob-exposed, runtime-visible.**
   An obvious default is allowed (and preferred), but it is never silent policy: the knob
   (§4) can flip it to reject, and the taken adaptation is discoverable at runtime (§4).
   Example: replay-stash miss on a thinking continuation → drop thinking for that turn
   (§5), never send a body known to be invalid.
4. **No adaptation → reject at the edge, in the client's dialect, before any round-trip.**
   `ErrorKind::ParseInput` re-encoded per §9 with the client dialect's error envelope and
   a status its retry logic will not retry.

**The boundary: carry the spec, not the water.** Rung 4's "brazen must know the upstream's
requirements" extends exactly as far as **structure** — the wire has no slot for the intent
(OpenAI-chat has no document-URL source; Ollama has no document slot at all). Provider
**policy** — value ranges, entitlement gates, model-conditional quirks, rate limits, the
restrictions a provider may lift next Tuesday — is *not* brazen's to pre-enforce: those
requests go through, and the upstream's own rejection propagates (§9). This is the existing
"brazen does not validate the long tail" stance (architecture.md §3.1) with its rationale
made explicit: wire shapes churn slowly and live in the mapping specs; policies churn
weekly and live in the provider's court. brazen carries every dialect's spec; it carries no
provider's water.

---

## 4. The lossy knob — one mechanism, runtime-visible

**One policy field, not a flag farm.** Rung 3's "always an exposed knob" multiplied naively
is a flag per lossy case. Instead the `[ingress]` table carries one field:

```toml
[ingress]
lossy = "adapt"                      # global default: "adapt" | "reject"
lossy_overrides = { thinking_replay = "reject" }   # per-case, keyed by adaptation name
```

Every lossy adaptation has a **name** (a stable snake_case identifier, e.g.
`thinking_replay`, `document_url_drop`), declared in the mapping spec that introduces it.
`lossy_overrides` keys are those names; an unknown name is a `Config` error (78) under the
row-style `deny`-adjacent stance — a typo'd override must not silently leave the default in
force. `"reject"` for a given case means rung 3 collapses to rung 4 for that case.

**Runtime exposure — the client can discover what happened, not merely be entitled to read
about it.** When a lossy adaptation fires:

- **Aggregate responses** carry a top-level `"brazen": {"adaptations": ["thinking_replay"]}`
  field. Dict-shaped clients see it; strictly-typed SDKs drop unknown fields harmlessly.
- **Streamed responses** carry an SSE **comment line** (`: brazen adaptation=thinking_replay`)
  before the affected chunk — comments are SSE-spec-legal, invisible to every conforming
  parser, and visible to `curl` and any debugging eye.

No new debug flag (architecture.md §2 non-goal, upheld); no logging subsystem. The
exposure rides the response itself.

---

## 5. The replay stash

**The problem.** Cross-dialect multi-turn breaks on opaque replay payloads: Anthropic
thinking `signature`/`redacted_thinking`, OpenAI Responses `encrypted_content`, Google
`thoughtSignature` must return verbatim on the next turn (architecture.md §3.1), but the
client owns the transcript and its dialect has no field to carry them. bl-61a9 gave these
payloads a canonical home *through* the hub; ingress must get them *around* the client.

**The mechanism: a fail-open XDG stash — a true cache, so statelessness survives.** On
`encode_response`, when a canonical event carries an opaque replay payload, the payload is
written to `$XDG_CACHE_HOME/brazen/replay/`. On `decode_request`, assistant turns are
joined back to their stashed payloads and the canonical request is recomposed with them
in place (thinking block re-injected before its tool call, exactly as the upstream
requires). Its **absence degrades fidelity, never correctness** (fail-open, below), which
is what keeps it a cache in the XDG sense and keeps brazen honestly stateless: no state is
*required*, so the architecture.md §2 exception list grows by one regenerable-in-spirit
entry (§13), not by a session store.

- **Key = what the client provably echoes.** The join key is the **tool-call id** for
  tool-bearing turns — upstream-generated, unique, and echoed by every conforming client
  in both the assistant message's `tool_calls` and the `role:"tool"` result. Conveniently,
  tool continuations are exactly the case where Anthropic *requires* the thinking block.
  For non-tool assistant turns the key is a **content hash** of the assistant text (the
  one stable thing the client echoes back). Id is the path: one file per key, no index, no
  manifest — `$XDG_CACHE_HOME/brazen/replay/<key>` holding the canonical-JSON payload
  block(s) for that turn.
- **Atomic, lock-free.** Write to a temp name, `rename` into place. One file per key means
  concurrent `bz` processes never contend on shared state. Reads of a missing file are the
  fail-open path, not an error.
- **Eviction.** Best-effort prune on write: entries older than 7 days are unlinked. No
  daemon, no manifest — the mtime is the record. A pruned entry is a stash miss, which
  fails open.
- **Fail open = degrade the knob that created the requirement.** On a stash miss for a
  thinking continuation, brazen does **not** send the body without the required block
  (a known, predictable 400 — forbidden by §3) and does not fail closed. It **omits
  thinking for that replay turn**: the request becomes valid without the block, the turn
  proceeds un-reasoned, and the adaptation is exposed as `thinking_replay` per §4. A
  client that would rather fail sets `lossy_overrides = { thinking_replay = "reject" }`.
- **One source of truth.** The stash is the *only* replay mechanism. brazen does not also
  smuggle payloads through extra response fields hoping the client echoes them — two
  carriage paths for one fact would drift, and the smuggle path fails invisibly on typed
  SDKs.

---

## 6. Routing & the `[ingress]` table

**Routing needs (almost) no new surface — the inbound model string resolves through the
existing ladder.** `decode_request` yields a canonical request whose `model` is whatever
the client sent; from there resolution is exactly architecture.md §4.3 / config.md §7: a
row owns the model via `model_aliases` or `model_prefixes`, substitution is
`model_aliases.get(model).unwrap_or(model)`, two owners is a `Config` error. A harness
hardcoding `"gpt-4o"` reaches Claude with one line of *existing* config on the Anthropic
row:

```toml
[[provider]]
name = "anthropic"
model_aliases = { "gpt-4o" = "claude-sonnet-4-6" }   # routes AND substitutes; no new mechanism

[[provider]]
name = "openai"
model_prefixes = []   # the built-in openai row owns gpt-* by prefix; clear it, or "gpt-4o" has two owners (78)
```

There is **no new precedence rung** and no ingress-side model table — a second
routing surface would be a second home for the model→row fact. The second row above
is the cost of that stance against the shipped defaults: an alias does NOT outrank a
prefix (they are one `row_owns` predicate, config §7), so masquerading a name the
built-in `openai` row's `model_prefixes` already claims needs the claim cleared —
one more line of *existing* config, still zero new mechanism.

The one genuinely new config surface is the `[ingress]` table (top-level, sibling of
`[[provider]]`, `deny_unknown_fields` like a row):

```toml
[ingress]
dialect = "openai_chat"       # REQUIRED to serve; the explicit no-sniffing selector (§2)
listen  = "127.0.0.1:4891"    # default shown; non-loopback REFUSES to start without `token`
token   = "..."               # optional bearer; when set, requests lack it -> 401 (§7)
lossy   = "adapt"             # §4; default "adapt"
lossy_overrides = {}          # §4
```

Severability holds: delete the `[ingress]` table and every ingress behavior is gone —
no core code path changes meaning. `--serve` with no `[ingress]` table is a `Config`
error (78) naming the missing table.

---

## 7. The listener: `bz --serve`

**A control-plane mode flag, not a verb** — the `--login`/`--list-models` family
(architecture.md §5.10.1): `bz --serve` short-circuits the one-shot data plane and enters
the accept loop. The listener is a **shell around the unchanged pipeline**: per request it
does `decode_request` → the ordinary `generate` (model-cache resolve → encode → auth →
send → frame → decode) → `encode_response`. Nothing inside `generate` knows it is being
served.

- **Process model: thread-per-connection, `std::thread`, blocking end to end.** No tokio,
  no async color — each connection thread runs one ordinary blocking pipeline at a time.
  architecture.md §2's "no in-process fan-out" is amended (§13) to say what it always
  meant: the *data plane* never fans out; N concurrent client connections are N
  independent pipelines, exactly as N `bz` processes would be. Connections are HTTP/1.1
  keep-alive: requests on one connection are served serially by its thread.
- **HTTP/1.1, hand-rolled, minimal, honest.** Request line + headers + `Content-Length`
  body in; status line + headers + body (or SSE / chunked) out. No TLS, no HTTP/2, no
  multipart. The clients are well-behaved SDKs on localhost; a reverse proxy owns
  anything fancier. This keeps the dependency set unchanged — a server framework is a
  deep dependency for a shallow need.
- **Testable by seam, like everything else.** The accept loop is written against a
  `Listener` trait yielding `impl Read + Write` connections; `main` wires
  `std::net::TcpListener`, tests wire in-memory duplex pairs. The 100%-coverage gate
  applies; only `main`'s wiring stays uncovered, as today (architecture.md §1).
- **Security posture.** Default bind is loopback. A non-loopback `listen` without `token`
  **refuses to start** (`Config`, 78) — a listener wired to the operator's credential
  store is an open door to a paid account, and an open door on a routable interface must
  be a deliberate, authenticated act. When `token` is set, a request without
  `Authorization: Bearer <token>` gets the dialect's 401 envelope. Client-supplied API
  keys are otherwise **ignored** — they are the client's fiction; upstream auth is
  brazen's own (row auth + CredStore), exactly as in one-shot mode.
- **Exit & signals.** The loop runs until SIGINT/SIGTERM; a mid-stream client disconnect
  kills that connection's upstream request (drop the transport read) and only that
  connection.

---

## 8. Pseudo-routes

The masquerade must answer the non-generation calls real harnesses make, or they fail
before the first request. Wave 1, `openai_chat` dialect:

- **`POST /v1/chat/completions`** — the data route (§2–§7).
- **`GET /v1/models`** — served **from the existing per-provider model cache**
  (model-discovery.md), re-encoded as the dialect's model list: the union of cached ids
  plus every `model_aliases` key of every row (the aliases are precisely the names a
  masquerade client is expected to ask for). Cold cache → empty `data` array, plus the
  aliases; brazen never lists upstream automatically (architecture.md §2), serving
  included — refreshing is the operator's `bz --list-models`.
- **Anything else** — the dialect's 404 envelope. No health route: `GET /v1/models` is the
  de-facto probe every OpenAI client already uses.

---

## 9. Error masquerade

**Carry the fact; re-encode it at the edge.** Upstream failures already arrive as in-band
canonical `Event::Error` with `ErrorKind`, HTTP status carried through `Frame.status`, and
`provider_detail` (architecture.md §3.4, AGENTS.md carry-the-fact rule). Ingress encodes
that into the **client dialect's** error surface so client retry logic keeps working:

- **Status:** the upstream status when one exists (the carried fact, never re-derived);
  otherwise the shared `ErrorKind`→status projection (the existing
  `ErrorKind::from_http_status` table read in reverse — one table, not two).
- **Envelope:** the dialect's error shape (`{"error": {"message", "type", "code"}}` for
  `openai_chat`), `type` mapped from `ErrorKind`, upstream detail preserved in `message`.
- **Mid-stream errors** (2xx stream that dies): the dialect's mid-stream convention —
  for `openai_chat`, an error chunk followed by stream end, matching what its SDKs
  tolerate.
- **Edge rejections** (rung 4, auth 401, route 404) use the same envelope with brazen as
  the origin; `ParseInput` maps to 400/`invalid_request_error` so no client retries it.

---

## 10. Streaming shape

The client's `stream` field controls the **client-facing response shape only**: `true` →
SSE re-encode of the canonical event stream as it flows; `false`/absent → the events fold
into one aggregate dialect body (the encoder's whole-body fold — the exact inverse of
egress `decode_full`'s explode-and-replay, and like it, no second code path: the aggregate
IS the stream, accumulated). The **upstream** shape follows brazen's own resolved `stream`
tri-state (config.md §4.2) independently — the canonical event stream is the pivot in both
directions, so any client shape composes with any upstream shape. A `stream:true` client on
a `stream:false` upstream simply gets its SSE frames all at once when the aggregate folds
through; correct, if unexciting.

---

## 11. The one-shot filter: `--in DIALECT`

Ingress dialect is a property of the **input edge**, not of the transport — so it also
composes with the ordinary POSIX filter: `bz --in openai_chat < request.json` reads one
client-dialect request from stdin and writes the client-dialect response to stdout
(aggregate by default; the request's `stream:true` selects SSE frames on stdout). Same
codecs, same ladder, same stash, no listener — this is what dissolves "listener vs filter"
into one capability with two front doors. `--in` names the dialect explicitly (never
sniffed, §2) and needs no `[ingress]` table (there is no listener to configure; `lossy`
defaults apply, overridable in the table if present — and a present table's
`lossy_overrides` names are validated exactly as on `--serve`: a typo'd name is a
`Config` error, 78, per §4, never a silently inert key). `--in` composes with `--raw=out`
like canonical input does; it is mutually exclusive with a positional prompt and with
`--raw=in` (64, flag conflict — each names a different input contract).

---

## 12. Wave scoping

- **Wave 1 (this spec's implementation balls):** `openai_chat` ingress — the lingua
  franca; it unlocks the largest client ecosystem. Codec pair, stash, `[ingress]` table,
  `--serve`, `--in`, `/v1/models`.
- **Wave 2 (shipped, bl-49bc):** `anthropic_messages` ingress (Claude-ecosystem
  tooling). Adds one codec pair (`src/ingress/anthropic_messages/`); §3–§10 machinery
  is reused untouched. The `decode_request` inverts the egress `POST /v1/messages`
  projection (anthropic-messages §2); the `encode_response` inverts the egress decode
  (§3), emitting the anthropic-native SSE **event framing** (`event: <name>` +
  `data: <json>` for `message_start` / `content_block_start` / `…_delta` / `…_stop` /
  `message_delta` / `message_stop`) and the `{"type":"error","error":{"type","message"}}`
  envelope. **Anthropic-specific narrowings discovered (documented, never silent):**
  - **The replay stash (§5) is IDLE for this dialect.** Anthropic natively carries
    thinking `signature`, `redacted_thinking`, and server-tool blocks in-band, so the
    encoder emits them as REAL wire content blocks (never stash writes) and the decoder
    reads the client's echoed blocks straight off the request. The wire `thinking` knob
    rides `extra` (there is no clean `budget→effort` inverse), so `req.reasoning` stays
    `None` and the §5 `thinking_replay` adaptation NEVER fires — the machinery is reused
    untouched, simply un-engaged. (An openai-speaking client reaching an Anthropic
    upstream still gets the stash; only an *anthropic-in* edge sidesteps it.)
  - **The error envelope carries no numeric status.** Anthropic's envelope names only a
    COARSE `error.type` family, so a specific upstream status (e.g. 503) projects to the
    nearest family (`api_error`) and re-decodes to that family's status (500). The precise
    status still rides the HTTP layer (`IngressState::status`, the listener's status line,
    §9); only the in-band `error.type` is coarse — the §4.2 table read in reverse.
  - **Two reverse projections are lossy** (the wire has one home for two canonical facts):
    the top-level `system` decodes wholly into `req.system` (a mid-transcript `Role::System`
    message the egress hoisted here cannot be recovered as distinct); and `EncryptedReasoningDelta`
    (an OpenAI-Responses concept) has no anthropic wire slot, so the encoder drops it.
  - **Under `--serve` the codec is reachable at the WAVE-1 openai-shaped routes** (§8 —
    `POST /v1/chat/completions`), not the native `POST /v1/messages`: routing is reused
    untouched (dialect-parametric pseudo-routes are a future ball). The `--in
    anthropic_messages` filter path (§11, no listener) exercises the codec fully today.
- **On demand:** `openai_responses`, `google_genai` ingress. Nothing in the design
  precludes them; nobody is asking yet (the empty-set rule — an unbuilt codec is the
  honest state).

---

## 13. architecture.md amendments (landed with this spec)

1. **§2 "No input-dialect auto-detection"** → "No input-dialect **sniffing**": canonical
   stays the default; `--in`/`[ingress]` name a dialect explicitly; structural sniffing
   remains forbidden forever. The old bullet's "no `--in-format`" sentence is superseded
   by this spec.
2. **§2 "Not stateful"** — the sanctioned-exceptions list gains the replay stash
   (`$XDG_CACHE_HOME/brazen/replay/`, §5): fail-open, prunable, absence degrades fidelity
   never correctness.
3. **§2 "No in-process fan-out"** — scoped to the data plane: one request per *pipeline*;
   the `--serve` shell may run N independent pipelines on N connection threads.
4. **§1 identity** — "one network round-trip per process" becomes "one network round-trip
   per **generation**"; `--serve` is a loop of generations, each individually holding the
   invariant.

## 14. Testing

- **Codec goldens, both directions:** dialect-request fixtures → canonical (decode), and
  canonical event scripts → dialect SSE/aggregate byte goldens (encode) — the mirror of
  the existing egress fixture suites.
- **The round-trip property:** for every egress golden, `decode_request(encode(req))`
  is identity on the canonical request (modulo defaults) — ingress and egress codecs
  check each other, no third source of truth.
- **Real-SDK drivers (the fidelity treadmill made loud):** an integration harness points
  the actual `openai` SDK at a `--serve` instance backed by a stub upstream; drift in
  chunk shape breaks these before it breaks a user. Live variants join the existing live
  test family.
- **Stash:** hit, miss→fail-open (adaptation exposed), miss+`reject` override, prune,
  concurrent atomicity (rename semantics).
- **Listener:** in-memory `Listener` seam — auth 401, non-loopback-without-token refusal,
  keep-alive serial requests, mid-stream disconnect, `/v1/models` cold/warm/aliases.
