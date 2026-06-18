# SSE / NDJSON decoder & `DecodeState`

> **Living document.** Edited like code. This spec owns the **transport-framing layer** behind `Protocol::framing()` and the **caller-owned decode state** every `Protocol::decode` threads. It derives from the architecture spec and MUST NOT contradict it; if it needs to, that spec changes first.
> **Derives from:** [Architecture & I/O Contract](architecture.md). **Depended on by:** [Anthropic messages mapping](anthropic-messages.md) §1.2/§3.2/§4.0, [OpenAI chat mapping](openai-chat-mapping.md) §3/§4.0.

---

## 1. Purpose & Scope

A `Protocol` declares **what framing its wire uses** as DATA (`framing() -> Framing`, architecture.md §4.1) and **decodes one already-parsed frame at a time** (`decode(frame, &mut state)`). This spec defines the layer between those two: the byte-chunk → `Frame` framers, the `Frame` and `Framing` types, the caller-owned `DecodeState`, and the determinism contract every decode must satisfy.

It owns, exactly and decisively:

- **`Frame`** (§3) — the one parsed unit handed to `decode`, identical across all three framings.
- **`Framing` + `framing().decoder()`** (§4) — the `Sse | Ndjson | Identity` enum (DATA) and how it constructs the right decoder for the peeked HTTP status.
- **`DecodeState`** (§5) — caller-owned open-block bookkeeping + cumulative usage + `terminated: bool`, and **why** it is caller-owned.
- **`SseDecoder`** (§6) — `push(chunk) -> Vec<Frame>`: blank-line frame split, `event:`/`data:` extraction, partial-frame and partial-UTF-8 buffering, recognition of both Anthropic `event: message_stop` and OpenAI `data: [DONE]` as terminal-marker payloads.
- **The NDJSON line-framer** (§7, Ollama) and **the Identity framer** (§8, `--raw`).
- **The whole-body / error-class frame** (§9) — how a non-2xx body reaches `decode` as one `Frame` without SSE grammar.
- **The adversarial-rechunking determinism contract** (§10) — the binding correctness property of this entire layer.

**Out of scope (owned elsewhere):** what events a frame decodes to and which `data.type`/`stop_reason` maps where (the mapping specs); the `run` loop, the single `Event::End`, the premature-EOF injection, exit codes, and signal handling (architecture.md §4.4, §5.6, §8); the `Sink`/output projections (architecture.md §5). This layer is **vendor-blind and event-blind**: it splits and buffers bytes into frames; it never inspects a frame's meaning.

### 1.1 Inherited invariants (restated so this spec is self-contained)

- **`decode` is PURE over `(Frame, &mut DecodeState)`** and object-safe; all cross-frame state lives in `DecodeState`, never on the impl, so a `Protocol` is shareable as `&'static dyn` (architecture.md §4.1).
- **`decode` NEVER emits `Event::End`.** `run` owns the single `End`, appended once after the body iterator drains (architecture.md §4.4, §3.4).
- **"Stream is over" is `DecodeState.terminated`** — set by `decode` when it consumes the provider terminal marker, NOT by bare EOF and NOT by the framer (architecture.md §3.5, §5.6, CR-9). **This spec's framer never sets `terminated`** (§6.5 makes this explicit and resolves the one ambiguity the task flagged).
- **Determinism under adversarial rechunking** is the central correctness property (architecture.md §9.3): identical input bytes, fed at any chunk boundary, decode to an **identical** `Vec<Event>`.

---

## 2. The one reframe: framing is DATA, the framer is the only stateful seam

Three wire shapes carry the same logical stream — SSE blocks, newline-delimited JSON, raw chunks — and each provider's `decode` is a pure state machine over one parsed unit. The reframe that dissolves the per-provider framing branch: **the framer's *only* job is to cut a byte stream at the right boundary and hand `decode` complete units.** It does not know events, does not know `terminated`, does not know which provider. `Framing` is a three-value enum on the `Protocol` (DATA); the matching framer is constructed by data, not by a vendor branch.

Because `decode` is pure and the framer holds the only cross-chunk *byte* buffer, the layer splits cleanly in two:

- **Byte buffering** (incomplete frame / partial UTF-8) lives in the **framer**, reset-free, owned by the `run` loop's local `decoder`.
- **Event-stream state** (open blocks, usage, `terminated`) lives in **`DecodeState`**, threaded by `&mut` into each `decode`.

Neither knows the other's internals. That separation is what makes both independently table-testable (architecture.md §9.2) and is the precondition for the §10 determinism contract.

---

## 3. `Frame` — the one parsed unit

A `Frame` is **what one `decode` call consumes**: a complete, framing-stripped payload plus the minimal envelope a protocol needs to dispatch. It is identical across all three framings — `decode` never asks "which framer produced this?"

```rust
/// One complete, framing-stripped unit handed to Protocol::decode.
/// Identical shape for SSE / NDJSON / Identity — the framing is spent producing it.
pub struct Frame {
    /// The SSE `event:` field value, if any. `None` for NDJSON, Identity, and
    /// SSE frames with no `event:` line (the OpenAI dialect). DATA the protocol MAY
    /// ignore: the mapping specs dispatch on the payload, not this (anthropic §3,
    /// openai §3). Carried so a protocol that wants it has it; never load-bearing here.
    pub event: Option<String>,
    /// The frame payload bytes: for SSE the concatenated `data:` value(s); for NDJSON
    /// one line (no trailing `\n`); for Identity one transport chunk verbatim; for a
    /// whole-body error frame (§9) the entire response body. Bytes, not str — a frame
    /// is handed across the framing boundary only when its UTF-8 is COMPLETE (§6.3),
    /// but the type stays `Vec<u8>` so Identity/--raw passes bytes through untouched.
    pub data: Vec<u8>,
    /// The HTTP status of a whole-body / error-class frame (non-2xx body, §9), or
    /// `None` for a streamed frame. The protocol's error parse (anthropic §4.0, openai
    /// §4.0) keys off `Some(_)` to know it is parsing an error envelope, AND derives the
    /// error kind from the status itself (`ErrorKind::from_http_status`) rather than
    /// reconstructing it from the body's error strings — the status is the authoritative
    /// fact and `run` already peeks it (architecture.md §4.1, §8). `Some(_)` is the old
    /// `whole_body` bit, now also carrying the value it always stood in for. The
    /// SSE/NDJSON grammars never set it.
    pub status: Option<u16>,
}

impl Frame {
    /// Identity / --raw: the chunk's bytes, written verbatim by RawSink (architecture.md §5.4).
    pub fn into_bytes(self) -> Vec<u8> { self.data }
    /// The payload as `&str` for a JSON parse. The framer guarantees complete UTF-8 for
    /// SSE/NDJSON frames (§6.3, §7); a protocol calling this on a malformed frame surfaces
    /// the error as ErrorKind::Provider via its own parse (the mapping specs), never a panic.
    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> { std::str::from_utf8(&self.data) }
}
```

**Why one `Frame` for three framings.** A protocol's `decode` dispatches on the **payload** (`data.type` for Anthropic, the chunk JSON for OpenAI, the line JSON for Ollama) — never on the framing that produced it. Keeping `Frame` uniform is the deep-narrow-interface rule (architecture.md house style): the framing detail is *spent* producing the `Frame`; nothing downstream re-derives it. `event` and `status` are the two thin envelope facts a protocol may consult; both are `None` on the common path.

**The terminal marker is a payload, not a `Frame` flag.** Anthropic's `event: message_stop` and OpenAI's `data: [DONE]` arrive as ordinary `Frame`s — `message_stop` as `{event: Some("message_stop"), data: b"{...}"}`, `[DONE]` as `{event: None, data: b"[DONE]"}`. The framer does **not** mark them terminal; `decode` recognizes them and sets `terminated` (§5, §6.5). This is the load-bearing resolution of "where is `terminated` set" — see §6.5.

---

## 4. `Framing` and `framing().decoder()`

```rust
/// Returned by Protocol::framing() — DATA, not behaviour (architecture.md §4.1).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Framing { Sse, Ndjson, Identity }

/// The object-safe framer the run loop drives. One local instance per request;
/// holds the only cross-chunk BYTE buffer. Never holds event state (that's DecodeState).
pub trait Decoder {
    /// Feed one transport chunk; return every COMPLETE frame it now yields.
    /// Returns `Vec<Frame>` (may be empty — a chunk that only extends an open frame
    /// yields nothing). Errors only on a structurally impossible state, never on a
    /// partial frame (which is buffered).
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, Error>;
    /// Called once after the transport body drains. Yields any final complete frame
    /// the stream left un-terminated by its boundary char (§6.4, §7.2). Identity has none.
    fn finish(&mut self) -> Result<Vec<Frame>, Error>;
}

impl Framing {
    /// Construct the framer for a SUCCESSFUL (2xx) stream. The status gates this:
    /// a non-2xx body bypasses framing entirely and is delivered as one whole-body
    /// frame (§9), so `decoder()` is only built on the streaming path.
    pub fn decoder(self) -> Box<dyn Decoder> {
        match self {
            Framing::Sse      => Box::new(SseDecoder::default()),
            Framing::Ndjson   => Box::new(NdjsonDecoder::default()),
            Framing::Identity => Box::new(IdentityDecoder),
        }
    }
}
```

`Framing` is the only enum this layer matches on, and it is a **map-of-three over data the protocol supplies** — not a vendor branch (the protocol already encodes its own framing choice; architecture.md §4.6 severability). Adding a protocol that reuses an existing framing adds **zero** code here; a genuinely new framing is one enum arm + one `Decoder` impl.

The provider→framing pairings (DATA on each `Protocol`, restated for reference):

| Protocol | `framing()` | Framer |
|---|---|---|
| `anthropic_messages` | `Sse` | `SseDecoder` (§6) |
| `openai_chat` | `Sse` | `SseDecoder` (§6) |
| `openai_responses` *(later)* | `Sse` | `SseDecoder` (§6) |
| `google_genai` *(later)* | `Sse` | `SseDecoder` (§6) |
| `ollama_chat` *(later)* | `Ndjson` | `NdjsonDecoder` (§7) |
| *(any, under `--raw`)* | `Identity` | `IdentityDecoder` (§8) — `run` forces this regardless of `proto.framing()` (architecture.md §4.4) |

---

## 5. `DecodeState` — caller-owned, the single home of cross-frame state

```rust
/// Caller-owned (lives as a local in the `run` loop, architecture.md §4.4).
/// Threaded by `&mut` into every `decode`. The shared shape; each Protocol uses the
/// fields it needs and ignores the rest (the empty case is not special — §3.1 of each
/// mapping spec scopes its slice).
#[derive(Default)]
pub struct DecodeState {
    /// In-flight content blocks keyed by canonical index — the single source of truth
    /// for "which block a delta routes to" and "which blocks are still open at finish."
    /// Opened on a block-start, removed on a block-stop. The OpenBlock payload is
    /// protocol-shaped (accumulated tool-arg JSON, thinking signature, redacted data —
    /// anthropic §3.2, openai §3.1); this spec owns the map, the mapping specs own the value.
    pub open: HashMap<u32, OpenBlock>,
    /// Cumulative usage as last revealed by the wire. Usage is cumulative on every
    /// provider (anthropic §3.6, openai §3.4), so this is a LAST-WINS-PER-FIELD snapshot,
    /// not an accumulator — re-emitted via Event::Usage as the wire restates it. Stored
    /// (not computed) because `Option` zero-vs-unknown is a real fact (architecture.md §3.5).
    pub usage: Usage,
    /// "Stream is over." Set TRUE by `decode` when it consumes the provider terminal
    /// marker (`[DONE]` / `message_stop` / `response.completed` / `{"done":true}` /
    /// a finishReason-bearing final chunk). NEVER set by the framer; NEVER set on bare EOF.
    /// `run` reads exactly this bit to decide the premature-EOF injection (architecture.md
    /// §5.6, CR-9). This is the one bit that distinguishes a clean end from a dropped stream.
    pub terminated: bool,
    /// Whether a SYNTHESIZED `MessageStart` has been emitted (openai §3.3). A protocol
    /// with a native message-start event (Anthropic) ignores this; one that synthesizes it
    /// on the first chunk gates emission on it. False until that first chunk.
    pub started: bool,
    /// Wire-positional block index → canonical content index (openai §3.1). Maps a
    /// positional namespace (OpenAI `tool_calls[].index`) onto the canonical index space so
    /// later argument fragments route to the block opened on first sight. Empty for protocols
    /// whose wire already speaks the canonical index (Anthropic). The next canonical index is
    /// COMPUTED from `open.len()`, never stored (single source of truth).
    pub tool_index: HashMap<u32, u32>,
    /// Accumulated `delta.refusal` text (openai §3.5), surfaced in the terminal
    /// `Finish{Refusal}`. Empty when no refusal field streamed.
    pub refusal: String,
}
```

**Why caller-owned (the load-bearing design choice).** If state lived on the `Protocol` impl, `decode` would mutate `&self` and the impl could not be shared as `&'static dyn Protocol` across the registry (architecture.md §4.1, §4.4) — every request would need its own boxed protocol, and `decode` would no longer be a pure function of `(frame, state)`. By putting *all* cross-frame state in a `&mut DecodeState` the `run` loop owns, each `Protocol::decode` stays a **pure state-transition function** — table-testable from literal frame sequences, trivially `Send + Sync`, and stateless between requests. The framer's byte buffer is the *only* other piece of per-request state, and it is likewise a local in `run`. Two states, two owners, both local to one stack frame: no global, no reset, no leak between requests.

**`terminated` lives here, not on the framer — and the framer never writes it.** This is the single source of truth for "stream over." The framer cannot set it correctly: a framer that flipped a bit on seeing `[DONE]`/`message_stop` would be duplicating the meaning that `decode` already assigns when it consumes that marker (architecture.md §3.5 forbids two homes for one fact). So the marker rides through as an ordinary `Frame` and `decode` — the one component that already understands the marker — sets `terminated`. See §6.5.

---

## 6. `SseDecoder` — the push/pull framer

The shared SSE framer for every SSE-framed protocol (Anthropic, OpenAI chat, and later OpenAI responses / Google genai). It buffers bytes across chunks and yields one `Frame` per SSE event block.

```rust
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,   // raw bytes received but not yet forming a complete frame.
}                   // The ONLY cross-chunk state. No `terminated`, no event awareness.
```

### 6.1 The SSE wire grammar (the subset brazen consumes)

An SSE stream is a sequence of **event blocks** separated by a **blank line**. Each block is one or more `field: value` lines:

```
event: content_block_delta\n
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}\n
\n
```

brazen consumes exactly three field kinds and ignores the rest:

- **`event:`** → `Frame.event` (the last `event:` line in a block wins; absent → `None`). The OpenAI dialect has no `event:` lines (openai §3); Anthropic always has one (anthropic §1.2) but `decode` dispatches on `data.type` regardless.
- **`data:`** → appended to `Frame.data`. **Multiple `data:` lines in one block concatenate with `\n` between them** (the SSE spec); a leading single space after the colon is stripped (`data: x` → `x`, `data:x` → `x`). On the brazen wire each frame is one `data:` line, but the multi-line rule is honored so the framer is a correct SSE consumer, not a line-matcher.
- **`:` (comment) and any other field** (`id:`, `retry:`) → ignored, contribute no bytes.

The **frame boundary is the blank line** (`\n\n`, or `\r\n\r\n` — `\r` is tolerated and stripped per line). Line splitting is on `\n`; a trailing `\r` on any line is dropped before field parsing.

### 6.2 `push` — split complete frames, buffer the partial tail

```rust
fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, Error> {
    self.buf.extend_from_slice(&chunk);
    let mut frames = Vec::new();
    // Repeatedly peel a complete block (terminated by a blank line) off the FRONT of buf.
    while let Some(end) = find_blank_line(&self.buf) {       // index past the `\n\n` / `\r\n\r\n`
        let block = self.buf.drain(..end).collect::<Vec<u8>>();
        if let Some(frame) = parse_block(block) {            // None for an all-comment/empty block
            frames.push(frame);
        }
    }
    // Whatever remains in `buf` is an INCOMPLETE frame — held until a future chunk
    // completes it. This is the partial-frame buffer (architecture.md §9.3).
    Ok(frames)
}
```

`find_blank_line` scans `buf` for the first `\n\n` or `\r\n\r\n`; `parse_block` splits the block into lines, strips trailing `\r`, applies §6.1's field rules, and returns `Some(Frame)` if the block carried any `data:` (or an `event:` worth surfacing), else `None` (a pure-comment keep-alive block contributes no frame). **An empty `data:` payload still yields a `Frame`** — emptiness is the protocol's concern (the mapping specs handle a zero-byte `data`), not the framer's.

### 6.3 Partial-frame and partial-UTF-8 buffering (what `MidUtf8` forces)

The framer operates on **bytes**, and a multi-byte UTF-8 sequence may be split across two transport chunks (`0xF0 | 0x9F 0x98 0x80`). The discipline that makes this safe:

- **Framing is a byte scan** (`find_blank_line` over `&[u8]`). `\n` (`0x0A`) cannot appear inside a UTF-8 multi-byte sequence (continuation bytes are `0x80–0xBF`, lead bytes `0xC0–0xFF`), so the blank-line scan is **byte-exact regardless of where a chunk was cut** — a chunk boundary in the middle of a multi-byte char never produces or hides a frame boundary.
- **`str::from_utf8` is applied ONLY to a complete frame's `data`** (by the protocol, via `Frame::as_str`, at decode time) — **never** to the live `buf`. Because a `Frame` is emitted only once its terminating blank line is in `buf`, and a blank line cannot fall inside a multi-byte sequence, **every emitted frame's `data` is a whole sequence of complete UTF-8 code points.** A chunk that cuts a char mid-sequence leaves the partial bytes in `buf`; they become valid the moment the next chunk arrives and the frame completes.

This is exactly why `Frame.data` is `Vec<u8>` and not `String`: the framer must hold raw bytes (possibly a UTF-8-incomplete tail) in `buf`, and only hand out complete frames. `MidUtf8` (architecture.md §9.3) is the rechunker that proves it — splitting a multi-byte sequence at the chunk boundary must not change the decoded events.

### 6.4 `finish` — the last frame and the final blank line

```rust
fn finish(&mut self) -> Result<Vec<Frame>, Error> {
    // Some servers omit the final blank line after the last block before closing.
    // If `buf` holds a complete block lacking its terminator, emit it; else drop a
    // trailing partial (a truncated frame is a premature drop — `run` handles it via
    // !terminated, architecture.md §5.6; the framer NEVER fabricates a terminal marker).
    if has_field_lines(&self.buf) { /* parse_block(self.buf.drain(..)) -> Frame */ }
    Ok(/* zero or one frame */)
}
```

The framer's `finish` flushes a final, blank-line-unterminated block (a real-world server quirk) — but it **never** invents a terminal marker and **never** touches `terminated`. If the buffered tail is a genuine partial (e.g. a half-received `data:` line from a dropped connection), it is discarded; `state.terminated` was never set, so `run`'s premature-EOF path fires correctly (architecture.md §5.6).

### 6.5 Terminal markers: framer splits, `decode` flips `terminated` (RESOLVED)

> The task flagged this as the one ambiguity to resolve cleanly. **Resolution, consistent with architecture.md §3.5/§4.4/§5.6 (CR-9) and both mapping specs (anthropic §3.8, openai §3.6):** the `SseDecoder` **splits** the terminal marker into an ordinary `Frame` and stops. It does **not** set `terminated` and does **not** treat the marker specially as framing.

Two terminal-marker shapes the SSE framer must produce correctly as frames:

- **Anthropic `event: message_stop`** — a normal SSE block (`event: message_stop\ndata: {"type":"message_stop"}\n\n`). The framer yields `Frame{event: Some("message_stop"), data: b"{\"type\":\"message_stop\"}", status: None}` like any other block. `decode` recognizes `data.type == "message_stop"`, emits `[]`, and sets `state.terminated = true` (anthropic §3.8).
- **OpenAI `data: [DONE]`** — the payload `[DONE]` is **not JSON**. The framer does **not** parse payloads (it never has — it splits bytes), so `[DONE]` rides through unchanged as `Frame{event: None, data: b"[DONE]", status: None}`. The framer needs **no special case** for `[DONE]`: it is just a `data:` value, and "parsing as JSON would throw" is a *decode* concern, not a framing one. `decode` sees the payload is the literal `[DONE]`, emits `[]`, and sets `state.terminated = true` (openai §3.6).

**Why the framer must not own `terminated`.** `terminated` means "the provider's terminal marker was consumed" — a *semantic* fact about the event stream, which only `decode` is positioned to assert (a future protocol's terminal marker is a `finishReason`-bearing chunk, architecture.md §3.4, which is indistinguishable from a content chunk at the byte level). Putting recognition in the framer would (a) require the framer to know each protocol's marker — re-introducing the vendor branch this layer exists to dissolve — and (b) create a second home for "stream over" alongside `decode`'s consumption of the marker, which architecture.md §3.5 forbids. So: **the framer is event-blind and writes only `buf`; `decode` is the sole writer of `terminated`.**

---

## 7. NDJSON line-framer (Ollama)

```rust
#[derive(Default)]
pub struct NdjsonDecoder { buf: Vec<u8> }   // bytes not yet forming a complete line.
```

One JSON object per `\n`-terminated line (Ollama; architecture.md §3.4, §5.2 input-side analogue). The framer is strictly simpler than SSE — the boundary is a single `\n`, there is no `event:`/`data:` field grammar:

```rust
fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, Error> {
    self.buf.extend_from_slice(&chunk);
    let mut frames = Vec::new();
    while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
        let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
        line.pop();                                  // drop the trailing `\n`
        if line.last() == Some(&b'\r') { line.pop(); }
        if !line.is_empty() {                        // skip blank lines (no frame)
            frames.push(Frame { event: None, data: line, status: None });
        }
    }
    Ok(frames)                                       // partial last line stays in buf
}
fn finish(&mut self) -> Result<Vec<Frame>, Error> {
    // A final line lacking its `\n` (server closed without a trailing newline) is a
    // complete frame; emit it. A genuine partial is discarded (premature drop -> !terminated).
    Ok(if self.buf.is_empty() { vec![] } else { vec![Frame { event: None, data: take(&mut self.buf), status: None }] })
}
```

The same partial-UTF-8 guarantee as SSE holds for the identical reason: `\n` cannot appear inside a multi-byte sequence, so a line emitted by `push`/`finish` is always complete UTF-8, and `str::from_utf8` (via `Frame::as_str`) runs only on complete lines. **The terminal marker `{"done": true}` is a normal line-frame** — the framer yields it like any other; Ollama's `decode` parses it, emits `Finish`/`End`-free `[]` or the finish event, and sets `state.terminated = true` (architecture.md §3.4). Same discipline as §6.5: the framer never recognizes the marker.

---

## 8. Identity framer (`--raw`)

```rust
pub struct IdentityDecoder;
impl Decoder for IdentityDecoder {
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, Error> {
        Ok(vec![Frame { event: None, data: chunk, status: None }])  // each chunk -> one Frame, verbatim
    }
    fn finish(&mut self) -> Result<Vec<Frame>, Error> { Ok(vec![]) }     // no buffering, nothing to flush
}
```

Under `--raw`, `run` forces `Framing::Identity` regardless of `proto.framing()` (architecture.md §4.4). The Identity framer is **stateless and lossless**: each transport chunk becomes exactly one `Frame` carrying the chunk's bytes verbatim, in arrival order. `run` wraps each into `Event::Raw(frame.into_bytes())` and `RawSink` writes the bytes verbatim, flushing per chunk (architecture.md §5.4). There is **no UTF-8 validation** (raw bytes may not be text), **no boundary scan**, and **no terminal-marker recognition** — under `--raw` the provider's own terminator stands and brazen appends no `Event::End` (architecture.md §5.4). `decode` is not called on the Identity path (`run` short-circuits to `Event::Raw`), so `DecodeState` is unused under `--raw` — consistent with `terminated` being decode-owned and `--raw` having no canonical "stream over."

---

## 9. The whole-body fold (non-2xx error body, and the non-stream 2xx success body)

A response that is **not a stream** is a single aggregate body the SSE/NDJSON grammar would never yield a frame from. There are two such cases, and the framing layer folds BOTH whole: the **non-2xx error body** (both mapping specs depend on it — anthropic §4.0, openai §4.0) and the **non-stream 2xx success body** (`stream:false`, config §4.2 — bl-24c2). The delivery split is keyed on the peeked status AND the carried streaming intent:

> **Contract (error body).** When `TransportResponse.status` is **non-2xx**, the `run` loop does **not** construct a streaming framer. It collects the entire response body and hands `decode` the **whole body as a single `Frame`** with `status: Some(resp.status)`. `decode` recognizes the whole-body error frame (`status.is_some()`) and parses the provider error envelope into `Event::Error(CanonicalError{kind, message, provider_detail})`, with **`kind` derived from that carried status** (`ErrorKind::from_http_status`), not the body's error strings (the mapping specs §4). The status that *selects* this path is the same status `decode` reads for the kind and `run` peeks for the exit code (architecture.md §4.1, §8) — one fact, one home.

> **Contract (non-stream 2xx body).** When the status is **2xx**, the path is **not `--raw`**, AND the carried streaming intent is **`!streamed`** (the resolved `stream:false`, config §4.2), the `run` loop again constructs **no** streaming framer. It drains the entire body and hands the bytes to `proto.decode_full(body, state)` — the protocol's whole-body success fold. `decode_full` is **not a second parser**: a non-stream body IS the aggregate the stream emits, so each protocol reconstructs the synthetic event sequence the stream would have produced and REPLAYS it through its OWN `decode`-internal helpers (explode→replay), yielding the SAME canonical `Vec<Event>` the streamed form would (message_start .. finish; `run` owns the one `End`). There is **no premature-EOF check** here — the body is complete, never a cut stream — and no framing, since the single JSON object is not a frame grammar.

```rust
// In run (architecture.md §4.4), refining the streaming loop for the whole-body cases:
let outcome = if !is_2xx(resp.status) && !raw {
    whole_body(resp.body, resp.status)                  // non-2xx error body → decode (status frame)
} else if is_2xx(resp.status) && !raw && !streamed {
    whole_body_success(resp.body)                       // §9 — drain whole → proto.decode_full
} else if raw {
    Framing::Identity.decoder().stream(resp.body)       // §8 — verbatim passthrough
} else {
    proto.framing().decoder().stream(resp.body)         // §4 — SSE or NDJSON framed stream
};
```

This keeps `decode` the single home of provider-error *parsing* (pure, fixture-tested, no network — architecture.md §8), while the framing layer owns only the *delivery* decision (stream-frame vs whole-body-frame), keyed on the peeked status. `frame.status` carries that same status into `decode`: `Some(_)` tells it to parse an error envelope rather than a streamed frame, and the value *is* the kind (via `from_http_status`) — so the status is read, never reconstructed from the body.

Because the kind comes from `frame.status`, the envelope **parse is best-effort**: a non-2xx whose body is non-JSON (a proxy's HTML, an empty 5xx) still yields `Provider{status}`, never `Transport` — the carried status is authoritative and is never dropped when the body fails to parse. An unparseable body simply degrades `message`/`provider_detail` (empty / `None`); the kind, exit, and `retryable()` are unaffected. A body that fails to parse is `Transport` **only** when there is no governing status to read (a mid-stream frame on a 2xx stream — the mapping specs §4.2).

**`--raw` non-2xx is the exception within the exception:** under `--raw` the body still streams verbatim through Identity (architecture.md §5.4) — the whole-body bridge applies only to the **normalized** (non-`--raw`) path, where `decode` runs and needs the error envelope as one frame. The raw 4xx/5xx exit code still comes from the peeked status (architecture.md §5.4) — that is `run`'s concern, not this layer's.

> **Note (resolved here; flagged for the coordinator).** architecture.md §4.4's `run` sketch constructs `framing.decoder()` and loops over `resp.body` **without** showing the non-2xx status gate; the gate is named only in the mapping specs' §4.0 as "owned by the SSE-decoder spec." This spec makes the gate explicit (above) as a refinement of the §4.4 sketch — it does **not** contradict §4.4 (which never claims framing is applied to a non-2xx body; it shows the happy path). No architecture.md change is required.

---

## 10. The adversarial-rechunking determinism contract (the binding property)

> **Contract (the central correctness property of this layer).** For any fixture, feeding its exact bytes through **any** chunking strategy and then through `framer.push(chunk)* ; framer.finish()` → `proto.decode(frame, &mut state)*` yields a **byte-identical** `Vec<Event>` and the **same** final `state.terminated`. The decoded event stream is a pure function of the input bytes, *independent of where the transport happened to cut them.*

The strategies (architecture.md §9.3), each a `Rechunker` over the same fixture bytes:

| Strategy | Cut points | What it stresses |
|---|---|---|
| `WholeFixture` | none — one chunk | the baseline; the oracle the others must equal |
| `OneByte` | after every byte | every possible mid-frame, mid-field, mid-char, mid-number boundary at once |
| `MidData` | inside a `data:` value | partial-frame buffering across a `data:` payload (§6.3) |
| `MidUtf8` | inside a multi-byte UTF-8 sequence | partial-UTF-8 buffering — proves `from_utf8` runs only on complete frames (§6.3) |
| `MidJsonNumber` | inside a JSON number (`"12"\|"34"`) | proves the framer never parses payloads — a split number is reassembled into one frame before `decode`'s parse, and a split tool-arg fragment stays a fragment (`JsonDelta`), never parsed mid-stream (architecture.md §3.6) |

**How the layer guarantees it.** The two states (§2) make determinism structural, not incidental:

1. **The framer emits a `Frame` only when it is complete** (terminating blank line for SSE, `\n` for NDJSON). A chunk boundary that lands mid-frame leaves bytes in `buf` and yields nothing extra; the frame emerges identically once completed. So the **sequence of `Frame`s is invariant** under any rechunking — `push` is associative over chunk concatenation: `push(a); push(b)` yields the same frames in the same order as `push(a ++ b)`.
2. **`decode` is a pure function of `(Frame, &mut DecodeState)`** with no hidden byte buffer of its own. Given the invariant `Frame` sequence and a `DecodeState` evolving deterministically, the `Vec<Event>` and final `terminated` are fixed.

Therefore the only way to break determinism is for the framer to (a) emit a frame before it is complete, (b) parse a payload (introducing a mid-fragment failure mode), or (c) carry event state in the framer — and §3/§5/§6 forbid all three. `MidUtf8` and `MidJsonNumber` are the adversarial witnesses that the forbidden behaviors are actually absent.

**Test shape** (architecture.md §9.2/§9.3 — pure, no network): a parametric test runs every committed fixture (`tests/fixtures/<name>.sse`, `<name>.ndjson`) through each `Rechunker`, asserts `decode_all(rechunk(fixture, strategy)) == decode_all(rechunk(fixture, WholeFixture))` for all strategies, and asserts the universal invariants (architecture.md §9.2): exactly one `run`-appended `End`; zero `End` from `decode`; every `ContentDelta.index` bracketed by a `ContentStart`/`ContentStop`; `Usage` fields `Option`; the first non-error event is `MessageStart{v == 1}`; and `state.terminated` set exactly once (on the terminal marker) for every clean fixture, never on a truncated one.

---

## 11. Module placement & line budget

Per architecture.md §11, the shared framer lives at `protocol/sse.rs` (`SseDecoder` + `NdjsonDecoder` + `IdentityDecoder`), and `Frame` / `Framing` / `Decoder` / `DecodeState` / `OpenBlock` live in `protocol/mod.rs` alongside the `Protocol` trait. All comfortably under the 300-line cap; if `sse.rs` approaches it, `NdjsonDecoder`/`IdentityDecoder` split to `protocol/ndjson.rs` without touching the trait or the determinism tests (the framers meet `run` only at the `Decoder` interface). The `Rechunker` strategies and the parametric determinism harness live under `tests/`.

---

## 12. Deliberate decisions (owned)

- **`Frame` is uniform across all three framings.** A per-framing `Frame` (an `SseFrame`/`NdjsonFrame`/`RawChunk` sum) would push the framing distinction past the layer boundary and force `decode` to match on origin — the opposite of the deep-narrow interface. One `Frame` with two thin envelope facts (`event`, `status`) keeps the framing spent at the boundary. The cost, owned: `Frame` carries an `Option<String> event` that the OpenAI/NDJSON/Identity paths leave `None` — a few unused bytes to avoid a type-level branch downstream.
- **The framer never parses payloads and never sets `terminated`.** It splits bytes. `[DONE]` needs no special case (it is just a non-JSON `data:` value the framer passes through); marker recognition and `terminated` are `decode`'s, the one place that already understands each protocol's marker (§6.5). This dissolves the "framer or decode owns `terminated`?" ambiguity in favor of `decode`, matching architecture.md §3.5/§5.6 and both mapping specs.
- **`finish` flushes a blank-line-unterminated final block but never fabricates a terminator.** A real server may omit the trailing blank line; `finish` recovers the last complete frame, but a genuine partial is dropped and `terminated` stays unset, so `run`'s premature-EOF path (architecture.md §5.6) fires correctly. The framer cannot turn a dropped stream into a clean one.
- **`--raw` bypasses `decode` and `DecodeState` entirely.** Identity is stateless and lossless; under `--raw` there is no canonical "stream over," consistent with `terminated` being a decode-owned, normalized-path-only fact.
