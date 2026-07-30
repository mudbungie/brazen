+++
title = "`-f -` reads stdin as a content-attach part — the portable, conventional spelling for the `-f /dev/stdin` trick that already works"
created = 1785390453
updated = 1785390453
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["ergonomics"]
+++
## The trigger

`bz "suggest something for dinner" | bz "tell me how to make"` looks like it should
work and doesn't: the second `bz` answers the prompt alone and the piped suggestion
is silently discarded. `src/pipeline/input.rs` `read_request`'s `Some(prompt)` arm
returns before touching the reader, per architecture.md §5.5:

> a positional prompt simply **wins**: any piped stdin is silently not consumed
> (the positional is the explicit signal — no sniffing, no "silent pick", and
> **no two-inputs error**)

No SIGPIPE fires either, because a short reply fits the 64 KiB pipe buffer — the
writer never blocks, exits 0, and the bytes are dropped when the pipe closes.

## The finding that scopes this ball

**The capability already exists.** `-f` takes a path, `/dev/stdin` is a path, and
`open_input`'s file/pipe parity means it already works on 0.0.4, unmodified:

    bz "name one dinner dish" | bz -f /dev/stdin "tell me how to make this"

Verified end-to-end against a local ollama row. `-f <(cmd)` works too. Repeatable
`-f` and `-f FILE` likewise already ship (architecture.md §5.5) — each file becomes
its own `Content::Text` part, `[file1, ..., fileN, prompt]`, not a concatenated string.

So this is **not** a new capability. It is sugar over one that exists.

## Scope — what to build

Teach `-f` the conventional `-` value, meaning stdin. Three justifications, none
of them capability:

1. **Portability.** `/dev/stdin` does not exist on Windows, and brazen ships
   `x86_64-pc-windows-msvc` + `aarch64-pc-windows-msvc` (ci.yml, release-plz.yml).
   Today this capability is POSIX-only by accident of implementation.
2. **Conventional spelling.** `-` for stdin is the unix norm (`cat -`, `tar -f -`,
   `sort -`, `gcc -`). `/dev/stdin` leaks an implementation detail into the UI.
3. **Discoverability.** Nobody reads `-f, --file <path>  attach a file's text as
   context` and infers `/dev/stdin`.

`-` is an explicit value NAMING stdin, exactly like a filename — not an absent
value with a fallback. That is what makes it compose with the repeatable form
with no extra rule.

## Implementation

- `src/pipeline/input.rs` `read_files`: a `PathBuf` of `-` reads the injected
  stdin reader instead of `fs::read_to_string`. Note `read_files` currently takes
  only `&[PathBuf]` — the reader must be threaded in, or the `-` case lifted to
  the callers (`run/mod.rs`, `run/count/mod.rs` — both use the same funnel).
- Errors unchanged: a read failure is exit 66 (`EX_NOINPUT`), same class as a
  missing file.
- Current behavior is exit 66 ``cannot read --file `-`: No such file or directory``,
  so the value is free — nothing that works today changes meaning.

## No special cases

- `-f - -f -` — stdin is read once; the second `-` hits EOF and yields an empty
  text part, exactly as `-f empty.txt` does today. Empty is the general path with
  no bytes, not a branch.
- Interactive tty — `src/main.rs:73` already swaps in `io::empty()` when
  `isatty(0)`, so `bz -f - "q"` at a terminal yields an empty part, never a hang.
- No interaction with the canonical-JSON stdin channel: that channel is the
  no-positional case, and `-f` is already refused alongside a piped canonical
  request (§5.5) and alongside `--raw`. Both refusals stand unchanged.

## Docs — part of this ball, not a follow-up

- `--help` line for `-f` must name `-`.
- `SKILL.md` worked example of the pipe chain.
- `README.md` + architecture.md §5.5 (the `-f` paragraph ~:822 and the input-source
  flags summary ~:965).

## Explicitly NOT in scope

Making bare `bz "prompt"` consume stdin implicitly, so the original chain works
with no flag at all. Considered and deferred: it reverses the §5.5 ruling, and it
makes `bz` eat inherited stdin in `while read` loops — the classic `ssh`-needs-`-n`
hazard, silent when it bites. Owner ruling: ship the explicit sugar first, since
`-f -` stays valid and harmless if the implicit default is later adopted, whereas
implicit-first cannot be backed out without breaking twice. File a separate ball
if the ergonomics still bite after dogfooding.