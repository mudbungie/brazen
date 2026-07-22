# Transport selection: application-wire vs transport-wire conformance

> Derives from [architecture.md](architecture.md) (§4.1 the transport seam, §9 testability) and
> [config.md](config.md) (§4 the provider row); must not contradict either. Sibling of
> [claude-code.md](claude-code.md) §3, which introduced the first non-HTTP transport kind.

---

## 1. Purpose & Scope

`WireRequest` carries method, URL, ordered headers, body and timeouts — and nothing else. The
`bz` binary rematerializes it through `src/native/transport.rs` (ureq + rustls), and **that
stack, not the `WireRequest`, decides a second set of server-observable facts**: which headers
are generated (`Host`, `User-Agent`, `Accept-Encoding`, `Content-Length`/`Transfer-Encoding`),
their casing and order, HTTP version and framing, connection reuse and `Connection:`/`TE:`
behaviour, ALPN, and the TLS ClientHello (cipher list, extension order and their contents —
the JA3/JA4-shaped fingerprint).

Therefore: **equal application requests do not imply equal transport wires.** No amount of
header or config editing makes ureq/rustls byte-identical to a client built on another runtime
(Bun/Node/undici, Go, Python, a browser). If an operator must present a *particular* HTTP/TLS
identity to an upstream, the only honest answer is to let that operator **own the HTTP
implementation** while keeping Brazen's canonical encode → auth → decode pipeline.

This spec (a) names the two conformance layers so the distinction stops being implicit, (b)
rules on the narrowest operator-selectable transport seam, and (c) pins the conformance harness
that proves each layer independently.

**Non-goals** (owned): no built-in vendor/client profile of any kind ships in Brazen — no
Claude-Code row, no impersonation preset, no compiled-in fingerprint; no system-prompt,
tool-list, beta-header, OAuth-policy or credential-ownership work; and **no claim** that
ureq/rustls can be configured into another runtime's ClientHello (§2.3).

## 2. The two conformance layers

### 2.1 Application-wire conformance

The layer Brazen owns and has always tested: after `encode` + `Auth::apply`, the `WireRequest`
— method, URL, the ordered `headers` list, the body bytes, the timeouts — equals a reference,
modulo a stated normalization (JSON key order, and any documented volatile field). This is what
`MockTransport` records and what `tests/sim_conformance.rs` and the live encode harnesses
assert. It says nothing about bytes on a socket.

### 2.2 Transport-wire conformance

What the *server* observes: the exact request head bytes (header names verbatim, in order,
including every header the client generated that never appeared in `WireRequest`), the HTTP
version and body framing, and the TLS ClientHello / ALPN offer. It is a property of the
**transport implementation**, not of the request.

**The rule this spec adds, stated once:** *semantic JSON equality is never "wire-identical".*
Any claim of wire identity must name its layer. §8's harness observes each layer with its own
instrument and is forbidden from inferring one from the other.

### 2.3 What is NOT claimed

Nothing here asserts that rustls can be driven to another stack's fingerprint. rustls does not
expose ClientHello cipher/extension ordering as configuration, and would not be believed if it
did without a capture. The seam's whole point is that this is **not Brazen's problem to solve
in-tree**: the operator brings the stack whose identity they need.

## 3. Mechanism ruling

Four candidates were weighed against: operator-owned (no vendor identity in-tree), single-shot
process semantics preserved, secrets never in argv/env/world-readable files, severable (removing
it deletes config, not core code), and testable.

| Candidate | Verdict |
|---|---|
| **A second built-in stack** (curl-impersonate / BoringSSL / a "profiles" table) | **Rejected.** Compiles a vendor's client identity into Brazen — the explicit non-goal — and doubles the dependency surface of the one impure module forever. |
| **Cargo feature selecting an alternate stack** | **Rejected.** Not operator-selectable: it is a *build* decision, so an installed `bz` cannot be pointed at a different stack, and it still ships the identity in-tree. |
| **`dlopen` a shared object implementing `Transport`** | **Rejected.** A C ABI over an iterator-of-`io::Result` seam is unsafe, un-versionable and untestable; it also puts foreign code in `bz`'s address space alongside live credentials. |
| **Point `base_url` at an operator-run local proxy** (available today, zero code) | **Rejected as the answer, documented as the fallback.** It works, but costs a daemon lifecycle and a listening port outside the single-shot model, needs the operator to re-terminate TLS, puts the credential on a loopback socket other local processes can observe, and leaves connect/idle budgets and cancellation unattached to `bz`'s own process tree. Still legitimate where a proxy already exists — it needs no Brazen feature and gets none. |
| **An external transport *program*, spawned per request, speaking HTTP over stdio** | **Chosen.** No daemon, no port; one child per generation preserves "one process, one round-trip"; the credential rides a private pipe; kill-the-child *is* cancellation; the spawn/stream/reap machinery already exists (`src/native/exec.rs`, claude-code §3); and it is severable — delete the row block and the capability is config-gone. |

**And it is not a new transport kind.** claude-code §3 already put a subprocess behind the
`Transport` seam via `WireRequest.exec`. There, the child **is the provider** (stdin carries the
dialect's prompt). Here the child **is the transport** (stdin carries the HTTP request message).
One spawn mechanism, two stdio envelopes — so `ExecSpec` grows one discriminator rather than
`WireRequest` growing a second subprocess field (§4.1). A row can therefore never be both, by
construction.

## 4. The seam

### 4.1 `ExecSpec.envelope` — what the child's pipes carry

```rust
pub struct ExecSpec { pub program: String, pub args: Vec<String>, pub envelope: Envelope }

pub enum Envelope {
    /// The child IS the provider: stdin = the dialect's own body, stdout = its own
    /// dialect stream, status 200 at spawn (claude-code §3.2). The default.
    Body,
    /// The child IS the transport: stdin = one whole HTTP/1.1 request message,
    /// stdout = one whole HTTP/1.1 response message (§5). Status is the parsed one.
    Http,
}
```

`wire.exec == None` remains the built-in ureq/rustls path, byte-identical to before. The
discriminator is read in exactly one place — the spawn's codec choice in the shim — never
matched on elsewhere.

### 4.2 Selection is a row block, operator-owned

```toml
[[provider]]
name = "my-adapter"          # an operator row in ~/.config/bz/config.toml — nothing ships
base_url = "https://api.example.com"
protocol = "anthropic_messages"
auth = "api_key"
api_header = { name = "x-api-key", scheme = "raw" }

  [provider.transport]
  program = "/opt/my-adapter/http-relay"   # PATH name or absolute path
  args = ["--profile", "reference-client"] # optional; operator's own vocabulary
```

- **Per row, never global.** A default row without the block is untouched, so "ordinary provider
  rows remain unchanged" is a structural fact, not a promise.
- **Whole-block `Option::or` across config layers**, exactly like `[provider.models]`
  (config §3.2) — a higher-precedence layer replaces the block, never merges it.
- **`transport` and `exec` are mutually exclusive**: a row carrying both is a resolve-time
  `Config` error (78) naming the row — a surfaced contradiction, the standard resolution rule.
  (`exec` already means "this row's child is the provider".)
- **No vendor knowledge:** Brazen never inspects `program`, never ships a row that sets it, and
  passes `args` verbatim. Any client identity lives entirely in the operator's program.

### 4.3 Where the delegate is stamped — one home

Transport policy already reaches the impure seam by riding the one struct that crosses it
(`wire.timeouts`, config §4.3). The delegate rides the same way, stamped by the same one call:

```rust
cfg.stamp_transport(&mut wire);   // sets wire.timeouts, and wire.exec when the row selects a delegate
```

It replaces the bare `wire.timeouts = cfg.timeouts()` at every stamp site — the shared
generation tail (`run::request::send`, which encoded and `--raw` input both pass through),
`run::models::fetch` and `run::count::fetch` — with a single home, so **every** request `bz`
makes on that row — generation, `--raw`, `--list-models`, `--count-tokens` — goes through the
selected transport by construction, with no fourth site to forget. The silent OAuth refresh
copies the whole transport policy onto its own token POST exactly as it already copies the
timeouts (auth §6), so the auth control request shares the row's transport identity; it remains
the separately documented exception to one-request-per-process and gains no new privilege.

## 5. The stdio HTTP envelope

The contract is HTTP itself — the thing being delegated — so there is no second vocabulary to
learn, version or misparse. **One request per child process.**

### 5.1 Request → child stdin

```
POST https://api.example.com/v1/messages HTTP/1.1<CRLF>
<each wire.headers entry, name: value, in order, verbatim><CRLF>
<CRLF>
<body bytes>                      then stdin is CLOSED
```

- **Absolute-form request target** (RFC 9112 §3.2.2 — what a proxy receives): the child needs no
  second URL channel, and Brazen never splits authority from path.
- **Only `wire.headers`, in order.** Brazen synthesizes nothing — no `Host`, no
  `Content-Length`, no `Accept-Encoding`, no `User-Agent`. Generating those is precisely the
  identity being delegated; a header Brazen invented here would be a header the operator's stack
  could not own.
- **EOF frames the body.** With one request per process, closing stdin is a complete and
  unambiguous terminator, so the envelope needs neither `Content-Length` nor chunking. A GET
  (`--list-models`) is the same path with an empty body — no special case.

### 5.2 Response ← child stdout

```
HTTP/1.1 429 Too Many Requests<CRLF>
retry-after: 5<CRLF>
<any other headers><CRLF>
<CRLF>
<body bytes, streamed>            until stdout EOF
```

- The head is parsed leniently and once: the status line's second whitespace-separated token is
  the status code; header lines split at the first `:`; `CRLF` and bare `LF` both terminate a
  line; the blank line ends the head. Everything after it is the body, **streamed incrementally
  from the first chunk** — bytes that arrived in the same read as the head are yielded, never
  buffered to end. A delegate that buffers a stream defeats the pipeline; the contract says so.
- Brazen keeps exactly the response header it already keeps: `retry-after`, verbatim (arch §3.3).
  Widening that set stays additive and is not done speculatively.
- The child **must not retry**: the generation invariant is one upstream request per `bz`
  process, and a delegate that retried would launder a retry past `bz`'s no-retry contract. It
  is the operator's obligation, stated in the contract and unenforceable from here — the same
  standing as "don't buffer".

### 5.3 Timeouts and cancellation

The resolved silence budget (config §4.3) reaches the child as environment variables — **not**
argv (visible in `ps`) and **not** pseudo-headers (which would pollute the header list the child
forwards verbatim):

| Variable | Meaning |
|---|---|
| `BZ_TRANSPORT_CONNECT_TIMEOUT` | whole seconds; cap on connection establishment |
| `BZ_TRANSPORT_RESPONSE_TIMEOUT` | whole seconds; cap on awaiting the response head |
| `BZ_TRANSPORT_IDLE_TIMEOUT` | whole seconds; inter-chunk stall bound |

Each is absent when unset — the empty set, never a sentinel. Honouring them is the child's job;
**enforcing** the idle bound is not delegated: `bz` applies the same inter-chunk stall bound to
the child's stdout it applies to a socket (claude-code §3.3), and a breach **kills** the child.
Cancellation intent is expressed the one way a process tree allows: `bz` kills and reaps the
child (on stall, on a dropped body iterator, on its own exit) — the existing `Drop` backstop, so
no zombie survives.

## 6. Errors, exits, secrets

The delegate maps onto the existing error model (arch §8) with no new kind and no new exit code:

| Failure | → |
|---|---|
| Spawn failure (missing/not executable) | `Transport` (69), naming program + OS error — the existing exec spawn error |
| Child produced no parsable status line (crashed, wrote garbage, exited before the head) | `Transport` (69), plus the child's stderr when it exited nonzero (existing exec fold) |
| Head parsed, body truncated mid-stream | the existing premature-EOF / `ensure_terminal` path (69) |
| Idle-budget breach | child killed → `Transport` (69), the same message shape as a stalled socket |
| Non-2xx status | **not** a transport failure: the parsed status flows on exactly as from ureq, through the one `http_error` fold and `ErrorKind::from_http_status` — 429 carries the parsed `retry-after` |

**Secrets.** The credential is in the request head, which travels one private pipe to one child.
It is never in argv, never in an environment variable, never in a temporary file, and never
logged: diagnostics for a malformed response name a *reason* ("no status line", "unterminated
head") and never echo head bytes — a broken or hostile delegate that echoed the request back
must not be able to induce Brazen to print the `Authorization` header. Child stderr is still
surfaced on a nonzero exit (claude-code §3.2): that is the operator's own program's output, and
the operator chose the program.

## 7. Severability

Delete the `[provider.transport]` block → the capability is config-gone, nothing recompiles.
Delete the `Envelope::Http` arm + its codec → the exec-as-provider kind and every HTTP dialect
are untouched. No flag, no verb, no canonical-model change, no new error kind, no default row.

## 8. The conformance harness

Two layers, two instruments, **no inference between them** (§2.2). Both instruments are
in-tree listeners, so the harness needs no network, no certificate, and no third-party binary.

### 8.1 Instrument A — the application observation

The normalized projection of what the server received: method, request **target
path**, the headers Brazen itself put on the `WireRequest` (lower-cased and sorted —
casing and order are transport facts, not application ones), and the body. Every
header the client stack generated is dropped, because none of it is the
application's. Equality of two application observations is application-wire
conformance (§2.1) and **nothing more**.

### 8.2 Instrument B — the transport observation

- **HTTP framing:** a plaintext `TcpListener` that accepts one connection and records
  the request head **verbatim** — request line (target form and HTTP version) and
  every header name in its exact casing and order, framing headers
  (`Content-Length`/`Transfer-Encoding`, `Connection`) included — then answers a fixed
  response. Both projections come off this one capture, and the difference between
  them is exactly the normalization: the harness never *infers* one layer from the
  other, and never calls the normalized form "wire-identical".
- **TLS ClientHello:** a `TcpListener` that reads the first flight, parses the
  ClientHello and records a JA3-form fingerprint (`version,ciphers,extensions,groups,
  point-formats`, GREASE dropped) plus the ALPN offer in order, then closes. No
  handshake is completed, so no server certificate and no TLS server implementation
  is needed — the first flight IS the fingerprint. The JA3 **digest** is deliberately
  not taken: the string diffs legibly in review, where a hash would only say "changed".

References are **captured files**, never compiled constants:
`tests/fixtures/transport/*`, re-taken with `BZ_CAPTURE=1 cargo test`, so a reference
is always a real observation. An operator conforming to *their* reference client
points the same harness at their own capture; Brazen ships no vendor's fingerprint.
The one foreign capture in-tree is a `curl`/OpenSSL ClientHello used solely to prove
the instrument discriminates, with its provenance recorded beside it.

### 8.3 What the harness proves

1. **The layers are independent (the spec's central claim).** One canonical request,
   run through the built-in transport and through a delegate, yields **identical**
   application observations and **different** transport observations. Asserted, not
   assumed: if it stopped being true, this spec would be wrong.
2. **The built-in transport's HTTP identity is pinned** against its committed capture
   — a ureq bump that changes generated headers, casing, order or framing is a
   reviewable fixture diff, not a silent change in what upstreams see.
3. **The built-in TLS offer is pinned — and is not even stable.** rustls deliberately
   **shuffles its ClientHello extension order per connection**, verified here across
   five connections: five different JA3 strings, one stable sorted offer. So the
   shipped stack has no fixed fingerprint to match *any* reference client, which is
   §2.3's claim demonstrated rather than asserted, and why the committed capture pins
   the sorted offer instead of a permutation.
4. **The fingerprint instrument discriminates**: a ClientHello captured from a
   different stack fingerprints differently — so the pin above cannot be vacuous.
5. **Every failure path maps** (§6), through the real binary and the real exit table:
   spawn failure (69, naming the program), no status line (69, echoing no head bytes
   and no credential), a delegate that dies (69, carrying its own stderr), a truncated
   body (69), a silence-budget kill (69, bounded in seconds), and a 429 that is *not*
   a transport failure — status 429 and `retry_after_seconds` reach the caller intact.
   Plus the happy path (exit 0), so the failures are about failure and not the seam.

The delegate the harness drives is `examples/stdio_transport` — the **reference
implementation of §5**, shipped as an example so it is documentation an operator can
read and run, and never installed by `cargo install`. It speaks plaintext `http://`
only, deliberately: the TLS stack is the operator's to bring, and the example's job is
to make the envelope legible without one in the way. The suite is `#![cfg(unix)]` —
the delegate is a spawned program and the failure stubs are `/bin/sh` scripts.

## 9. Summary of decisions

1. **Two named layers.** Application-wire (the `WireRequest`) and transport-wire (what the
   server observes). Equality at one never implies the other; "wire-identical" must name a layer.
2. **The operator owns the stack, not Brazen.** No built-in profile, no impersonation table, no
   claim about rustls' fingerprint.
3. **An external transport program over stdio**, chosen over a second built-in stack, a cargo
   feature, `dlopen`, and a local proxy (§3) — the proxy stays the documented no-feature fallback.
4. **Not a new transport kind:** `ExecSpec.envelope` discriminates "the child is the provider"
   from "the child is the transport"; a row cannot be both.
5. **Selection is a per-row `[provider.transport]` block**, mutually exclusive with `exec`,
   folded whole like `[provider.models]`.
6. **The envelope is HTTP/1.1 itself**, absolute-form, headers verbatim with nothing
   synthesized, EOF-framed both ways, one request per child.
7. **Timeouts by environment, cancellation by kill**; the idle bound stays Brazen's to enforce.
8. **One stamp home** (`stamp_transport`) replaces four duplicated timeout stampings, so every
   request kind — including the OAuth refresh — inherits the row's transport by construction.
9. **Secrets ride the pipe only**; no argv, no env, no temp file, no echo in diagnostics.
10. **The harness proves both layers separately**, with a committed capture as the reference and
    an example program as the delegate and as the operator's documentation.
11. **rustls shuffles its ClientHello per connection** (measured, §8.3): brazen's own stack has no
    stable fingerprint at all, so "configure ureq/rustls to look like X" was never on the table.
