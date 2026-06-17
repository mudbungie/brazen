# Protocol-layer dedup: shared JSON helpers + synthesized-stream mechanics

Derives from `architecture.md` and `sse-decoder.md`. Task `bl-54a4`.

## Why

The line-split refactor (`bl-78f7`/`bl-ec6b`/`bl-d283`) reorganized each protocol
`decode` into `mod`/`blocks`/`errors` but did **not** dedup across providers. Each
decoder carries its own copies of the same leaf JSON accessors, and the three
*synthesized-structure* decoders carry byte-identical copies of the same
drain/index mechanics. Two representations of one fact drift; this spec collapses
each fact to one home.

The canonical model (`CanonicalRequest`/`Event`) is already the consolidated core;
every protocol is a *lossy projection* onto it. We consolidate **only** the
mechanics the canonical model *implies* — drain order, index discipline, JSON
access — and never the wire shapes, which **are** the divergence. A shared engine
that eats wire-shape variation would become a second source of truth that drifts.

## The two families

- **Explicit-structure** (`anthropic`, `openai_responses`): the wire carries block
  start/stop; the canonical index keys off a wire field (`index`) or a wire pair
  (`(output_index, content_index)` → `part_index`).
- **Synthesized-structure** (`openai` chat, `google_genai`, `ollama_chat`): no block
  markers — the decoder synthesizes `MessageStart`/`ContentStart`, opens a lazy text
  block, assigns a monotonic `open.len()` index, and drains every still-open block at
  the terminal frame. `google` and `ollama` `blocks.rs` came out nearly verbatim.

## Deliverable 1 — shared leaf JSON accessors (`src/protocol/json.rs`)

Pure functions over `&Value`/`&[u8]`, zero divergence risk, used by **both** families
and by `encode`. Replaces the per-provider `pub(super)` copies.

| fn | signature | was duplicated in |
|----|-----------|-------------------|
| `parse` | `(&[u8]) -> Result<Value, CanonicalError>` | openai, ollama, google, openai_responses (decode); inlined ×2 in anthropic |
| `text_of` | `(&Value, &str) -> String` | anthropic, openai, google, ollama, openai_responses |
| `nonempty` | `(&Value) -> Option<&str>` | openai, google, ollama |
| `u32_at` | `(&Value, &str) -> u32` | openai_responses (`u32_at`) + anthropic (`index` ≡ `u32_at(v,"index")`) |
| `to_json_string` | `(&Value) -> String` | openai & openai_responses (encode); google & ollama (decode) |

`anthropic`'s `index(v)` collapses into `u32_at(v, "index")` (same body). The two
`encode` copies of `to_json_string` fold in too: it is a leaf accessor (a `Value` →
its JSON string), **not** encode structure — folding it does not couple the per-wire
encode bodies, which stay independent (see Scope guard).

## Deliverable 2 — fit-test (GO/NO-GO), then the safe mechanics

### The GO/NO-GO gate: fit `openai_chat` into a shared flow skeleton — **NO-GO**

The richer idea was a shared *synthesized-stream decoder skeleton* that **drives the
flow** (`MessageStart` once → content → terminal drain → `Usage`/`Finish`) and calls
per-provider extractor fns for the wire-shaped parts. The gate: fit `openai_chat`
into that skeleton on paper. `openai` is a **cousin, not a twin** of `google`/`ollama`:

1. **Tool args.** `google`/`ollama` deliver each tool call **whole** → one
   `ContentStart{ToolUse}` (synth id `call_{index}`) + a SINGLE `JsonDelta`.
   `openai` streams tool args as **fragments** accumulated into `OpenBlock.buffer`,
   with the id/name appearing only on the first fragment.
2. **Index namespace.** `google`/`ollama` open a new block at the bare `next_index`
   every tool call. `openai` keys off the wire's own `tool_calls[].index` through the
   `tool_index` map (a new canonical block only on first sight of a wire index).
3. **Terminal flow.** `google`/`ollama` emit `… ContentStop* → Usage → Finish →
   terminated` in **one** frame. `openai` drains + `Finish` at the
   `finish_reason` frame, takes `Usage` from a **separate later** frame, and flips
   `terminated` only at the still-later `[DONE]` marker.

Folding `openai` into one flow-driving skeleton forces it to branch on bool flags —
`tool_args_are_whole`, `text_is_fragmented`, plus terminal-flow flags
(`usage_in_finish_frame`, `terminal_is_done_marker`). That is the abstraction leaking
into a config-language: a second source of truth keyed on wire shape. **ABORT** the
flow skeleton. Each provider's `decode`/content/tool-call/terminal flow stays
independent.

### What we *do* build: the canonical mechanics (`src/protocol/synth.rs`)

The abort is specifically of a *flow engine that knows wire shapes*. It is **not** a
reason to leave byte-identical, wire-agnostic mechanics copied three times. Three
primitives are pure functions of `(&mut DecodeState, &mut Vec<Event>)` — they carry
**no** wire knowledge and are identical across all three synthesized decoders — so
they are leaf utilities like `text_of`, not a skeleton:

| fn | signature | what it owns | used by |
|----|-----------|--------------|---------|
| `next_index` | `(&DecodeState) -> u32` | index discipline: the dense `0..n` = `open.len()`, never stored | openai, google, ollama |
| `open_text` | `(&mut DecodeState, &mut Vec<Event>) -> u32` | the lazy text block: find-or-create, return its index | openai, google, ollama |
| `drain` | `(&mut DecodeState, &mut Vec<Event>)` | drain order: every open block → `ContentStop` ascending, removed | openai, google, ollama |

Each provider keeps its own flow and **calls** these three. `open_text` subsumes
`openai`'s `text_index` helper; `drain` replaces the identical sort/remove/`ContentStop`
loop in all three terminal handlers. There is no flag, no generic content/tool-call
handling, no flow inversion — the wire-shaped parts (tool-call wholeness, the
`tool_index` map, `Usage`/`Finish` placement, `[DONE]`) stay where they diverge: in
the provider. This is the maximal **safe** dedup: 3-of-3 on the canonical mechanics,
0 wire shapes consolidated.

## Scope guard

- `encode` bodies stay per-wire (keys/nesting/renames diverge); the only shared thing
  is the `to_json_string` leaf accessor (D1) and the existing text-only-slot/`slot_err`
  pattern, both already local where used.
- The explicit-structure decoders (`anthropic`, `openai_responses`) keep their own
  flow and wire-index discipline (`index` / `part_index`); they share only the D1 leaf
  accessors, never `synth.rs` (which is meaningless without synthesized structure).

## Close gate

100% line coverage, no `*.rs` over 300 lines, `clippy -D warnings`, fmt — the repo
`make check` gate, unchanged. A pure refactor: the existing decode tests are the
behavior contract and must pass untouched.
