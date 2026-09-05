+++
title = "a `grep -q` fed by a pipe answers FALSE when it matched: the leak self-test's flake, and one site where the gate drops a finding"
created = 1788583954
updated = 1788583955
claimant = "Animations-X"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
The leak-scan stack here is a near-byte-identical port (bl-39f1), so brazen
carries the same defect at the same sites. Found, measured and fixed in lernie
(its bl-7ad6) and in yog (bl-e33a).

## The defect

`printf … | grep -q PATTERN` is a race under `set -o pipefail`, not a style
choice. `grep -q` exits the instant it matches and closes the read end of the
pipe; the writer is then killed by SIGPIPE part-way through its own write, and
`pipefail` takes the pipeline's status from that DEAD WRITER rather than from
the reader that answered. **The pipeline reports failure exactly when the
pattern MATCHED.** `PIPESTATUS` at a false answer reads `141 0`.

It is a flake only while the subject fits one write into the pipe buffer. Where
the writer outruns it — a `find`, a `grep` over a file — the false answer is
not rare but certain.

## Where it bites here

- **`scripts/leak-selftest.sh`, three sites** — the `-qF` content check, the
  `":$ln  ["` anchor check and the `-qi` FIXTURE_MARKER check. All three are
  `… || { report; fails=1; }`, so a false answer **reports a live rule dead**:
  one `make check` fails with `self-test: [<rule>] line N of <fixture> was NOT
  flagged` and the next passes on the same tree. A self-test an agent learns to
  re-run rather than read is how a genuinely dead rule gets waved through.
- **`scripts/leak-scan.sh`, `scan_paths`** — and this one is not a flake. The
  shape is `… | grep -qE "$FORBIDDEN_PATH" && printf …`: a false answer means
  the `&&` never fires, so **a real credential-shaped path is reported by
  nobody**. The gate misses a finding, in silence.
- **`scripts/smoke.sh`, two sites** — both negated (`! printf … | grep -qF`),
  so a false answer asserts the framing is ABSENT precisely when it was
  present, and the passthrough beat passes on a stream that failed its own
  discriminator.

`.githooks/pre-commit` is POSIX `/bin/sh` and its `find src | grep -q .` is out
of scope: dash has neither the option that makes the shape wrong nor the
herestring that fixes it.

## The fix

A `grep -q` reads its subject from a **herestring**, never from a pipe:
`grep -qE PATTERN <<<"$subject"`. There is no second process to die, under
either setting of `pipefail`. Semantics are byte-identical and bash has had
`<<<` since long before 3.2, so the macOS leg is unaffected.

The ban is on the **shape**, not on the option, because a sourced file cannot
see whether its caller set `pipefail` — `leak-selftest.sh` does not set it and
inherits it from the scanner that sources it, which is how the defect reached
the one file whose whole job is to prove the gate is not lying.

## The guard

At the foot of `self_test`, as one more arm of the same two-direction
discipline: the fixtures prove a rule still bites, and this proves that the
answer a pipeline reports is the answer its reader gave. It holds every tracked
BASH script under `scripts/` and `.githooks/` to the rule, skips a `#!` naming
another interpreter, treats a file with no `#!` as a sourced bash fragment (in
scope), and fails outright if it enumerates nothing. Whatever scans for the
shape must be self-immune: write the pipe as `[|]`, the idiom `leak-rules.sh`
already uses for `Fil[e]`.