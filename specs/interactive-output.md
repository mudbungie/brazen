# Interactive output — TTY-only pretty text skin

Derives from [architecture.md](architecture.md) §5 (the I/O & POSIX contract). A
strictly-additive skin over the default `--text` projection that activates **only**
on an interactive terminal and leaves every byte of the building-block contract
untouched. `--json` and `--raw` are machine contracts and are NEVER prettified.

## 1. The invariant this preserves

brazen is a unix filter: `bz "q" | tool`, `$(bz "q")`, `bz "q" > file` must capture
**exactly today's `TextSink` stdout bytes** — the `ContentDelta::TextDelta` text,
raw, per-delta flush, no injected trailing newline (§5.3). That is the sacred
building-block contract. Pretty mode must not bend it: the answer on stdout stays
byte-identical and unstyled even on a tty; all human chrome goes to **stderr**.

So redirecting stdout on a tty (`bz "q" > file`) still yields a clean answer, while
the human at the terminal still sees tool calls and a footer on stderr — the
curl idiom (body on stdout, meta on stderr).

## 2. The seam — the lib stays pure; the shim contributes one bool

The pure lib MUST NOT call `isatty`. There is already a precedent: `Args.tty`
carries the **stdin**-isatty fact from the shim (`bz/src/main.rs` `stdin_is_tty()`,
the §5.5 bare-invocation hint) into the lib. The **stdout**-isatty fact rides the
same bundle the same way:

```
Args.stdout_tty: bool   // #[cfg(unix)] libc::isatty(STDOUT_FILENO)==1, the sibling
                        // of stdin_is_tty(); false on non-unix and on a pipe
```

The shim's whole contribution is that one bool. **All policy lives in the lib**: a
pure `Style::resolve(stdout_tty, &EnvSnapshot) -> Style` decides pretty-vs-plain and
owns every glyph/SGR. It is table-tested to 100% with zero IO. The shim probes
`isatty` and nothing more — `make cov` keeps `bz/` coverage-excluded, so the one
impure line never needs a test.

## 3. Activation predicate (`Style::resolve`)

Pretty is ON **iff all** hold:

- `stdout_tty` — stdout is an interactive terminal (the shim's bool), **and**
- `output == OutMode::Text` — the default text projection only; `--json`/`--raw`
  never construct this sink, **and**
- `NO_COLOR` is **unset** — any value (incl. empty) set ⇒ off (the de-facto
  convention: presence, not truthiness), **and**
- `TERM` is **set and ≠ `"dumb"`** — a real terminal type.

Otherwise `Style::PLAIN`, which renders **byte-for-byte the current `TextSink`**.
There is **NO `--color` flag** — a new flag is a smell; `NO_COLOR` + `isatty` + `TERM`
is the complete conventional predicate. Pretty is a skin, never a fork: PLAIN is the
general path, pretty adds escapes and stderr chrome on top.

**Glyph degradation.** Glyphs are UTF-8 (`⚙`, `✓`); when the terminal's encoding is
uncertain — `LC_ALL`/`LC_CTYPE`/`LANG` does not name a UTF-8 locale — they degrade to
ASCII (`*` tool, `+` footer, `x` error). Color and glyph fall under one predicate: PLAIN
is no color and no chrome at all; pretty is color + UTF-8 glyphs, or color + ASCII glyphs
under a non-UTF-8 locale. One predicate, total fallback, no half-states.

## 4. The stdout / stderr split (the contract)

| Channel | Carries | Styling |
|---------|---------|---------|
| **stdout** | the answer bytes — the `TextDelta` text, and under `--thinking` the reasoning + the one `\n` separator | answer NEVER styled; thinking wrapped in dim SGR (see §6) |
| **stderr** | all chrome: tool-call lines, the finish/usage footer, styled errors | colored sigil gutter |

stdout is the §5.3 `TextSink` output, exactly. stderr is new, human-only, and empty
on a pipe (PLAIN writes no chrome). The §5.9 rule still holds — `Event::Error`
already lived on stderr in text mode; pretty only restyles it.

## 5. Event → render mapping (Sigil gutter)

Each stderr chrome line begins with a single colored glyph gutter. Only widely-safe
SGR: `\x1b[2m` dim, `\x1b[1m` bold, `\x1b[3Nm` fg, each closed by `\x1b[0m`.

- **Tool call** (today silently dropped in text mode — the real win). On
  `ContentStart{ToolUse{name}}` open a pending tool line; accumulate the streamed
  `JsonDelta` args fragments; on the matching `ContentStop` flush one stderr line:
  a yellow `⚙` (ASCII `*`) gutter, the tool **name** bold, then the args dim.
  Example: `⚙ get_weather {"city":"SF"}`. **An open tool block also flushes at the
  terminal** — at the start of `Event::Error` (so a mid-stream-truncated call reads
  `⚙ name {partial` *then* the red `✗`) and on `Event::End` (the universal net) —
  because the streaming-drop / premature-EOF paths (architecture.md §4.4/§5.6) emit
  **no** `ContentStop` to close the block, so flushing only on `ContentStop` would
  drop a tool call cut mid-stream. The flush is idempotent (it no-ops once the block
  is taken), so the clean `ContentStop` path is unchanged.
- **Footer** on `Finish` (carrying `Usage` seen on the run): one dim stderr line —
  a green `✓` (ASCII `+`) gutter, the finish reason, then the token counts. Example:
  `✓ stop · 312 in · 47 out`. Cache counts append only when present and non-zero
  (`· 10 cache_r`); a `None`/zero counter is omitted (never `0`, which would lie).
  `Finish` is the trigger; `Usage` is buffered as it arrives.
- **Error**: `Event::Error` → stderr, the message after a red `✗` (ASCII `x`) label.
  Exit-code behavior is **identical** to plain (the §8 mapping is `pump`'s, untouched).

**NO model/role header** — low signal, omitted by the chosen mockup. **NO spinner** —
a "waiting for first token" animation needs a background thread racing the blocking
transport; not worth the complexity (a follow-up ball may note it as a future nicety).

## 6. `--thinking` — dim on **stdout**, not a stderr line

`--thinking` keeps its §5.3 channel and content **exactly**: the reasoning text on
stdout, then the one-shot `\n` separator, then the answer — in BOTH tty and pipe. In
pretty mode the only difference is that the thinking text is wrapped in dim SGR
(`\x1b[2m…\x1b[0m`); the `\n` separator and the answer are unstyled. So:

- channel/content of `--thinking` is mode-invariant (stdout, same bytes modulo SGR),
- the **answer** is unstyled on stdout in every mode,
- pretty's only stdout effect is the dim escapes bracketing the thinking text.

This deliberately overrides the design-mockup line that put thinking on a stderr
gutter: thinking is dim-on-stdout, not stderr chrome. Only tool calls + the footer +
errors are stderr chrome.

## 7. Layout & testability

- `src/pipeline/style.rs` — the `Style` capability, `Style::resolve(stdout_tty, env)`,
  and the SGR/glyph helpers with ASCII fallback. Pure; the seam the shim feeds.
- `src/pipeline/pretty.rs` — the pretty `Sink`: holds `out`, `err`, `style`, the
  thinking `pending_sep` (as `TextSink`), and the per-tool-block accumulator. Its
  stdout path is the same logic as `TextSink` so the answer bytes are provably
  identical.
- `src/run/mod.rs` — in the `OutMode::Text` arm: `Style::resolve(args.stdout_tty,
  env)`, then pick `PrettySink` vs `TextSink` on `style.is_pretty()`. The plain
  branch is the literal current code.

**Tests (the contract):**

- `Style::resolve` — exhaustive table over `tty × mode × NO_COLOR{unset,empty,set} ×
  TERM{unset,dumb,xterm} × locale{utf8,c}` → expected `Style`; 100%, zero IO.
- `PrettySink` — golden `Event` streams through `(Vec<u8> out, Vec<u8> err)`; assert
  BOTH byte-for-byte: plain answer, a tool call (stderr name-bold/args-dim line),
  thinking (dim-on-stdout + separator + answer), the footer, a styled error, ASCII
  degradation, and the PLAIN fallback.
- **The regression test:** for a stream WITHOUT `--thinking`, `PrettySink.out` bytes
  are **identical** to `TextSink.out` bytes — chrome only diverges on stderr. For the
  `--thinking` stream, the answer-portion bytes match and stdout differs only by the
  dim SGR around the thinking text. This is the building-block contract, asserted.
