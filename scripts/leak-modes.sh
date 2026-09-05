#!/usr/bin/env bash
# leak-modes.sh — the two SCOPES the scanner is asked in (bl-1007), split from
# the mechanism at the 300-line cap (bl-7547). Sourced by `leak-scan.sh`, never
# executed: it defines no CLI verb and reads `$FIXTURES` and every `scan_*`
# primitive from the file that sources it.
#
# The seam is the same one `leak-selftest.sh` is on. `leak-scan.sh` is HOW a
# file is judged — the rule loop, the exception filter, the truncated finding.
# This is WHICH files get judged, and materializing them is a subject of its
# own: an index checkout, an archive extraction, a scratch directory with a
# trap on it. Neither half knows the other's business, and the self-test
# harness spends only the first.
#
# TWO SCOPES, BECAUSE THEY ANSWER DIFFERENT QUESTIONS. The tree mode asks "does
# this checkout carry a finding" — the right question for a commit hook (the
# tree IS your change) and for the workflow that judges the published ref. The
# `--commit` mode asks "does this OP publish a finding", which is the author's
# own text at the moment of writing, and it is what a store gate wants. The
# scanner's own head carries the full reasoning, including why the tree
# question is the wrong one for a shared, long-lived checkout.

# scan_set FILE... -> 0 clean, 1 with findings. The file-list scan every mode
# shares: a rule's own fixture is judged by every rule BUT its own (see
# leak-scan.sh's header), so one file cannot mean two things depending on which
# mode read it.
# `${a[@]+"${a[@]}"}`, not `"${a[@]}"`: under `set -u` bash 3.2 treats the
# expansion of an EMPTY array as an unbound variable, kills the shell mid-scan
# — and exits 0 doing it, so a tree full of findings passed the gate on macOS,
# which ships 3.2 and always will (bl-1015). The guard is the portable idiom
# for "expand this array, or nothing".
scan_set() {
  local files=() fixtures=() f rc=0
  for f in "$@"; do
    case "$f" in "$FIXTURES"/*) fixtures+=("$f"); continue ;; esac
    files+=("$f")
  done
  if [ "${#files[@]}" -gt 0 ]; then scan "${files[@]}" || rc=1; fi
  for f in ${fixtures[@]+"${fixtures[@]}"}; do
    f="${f##*/}"
    scan --skip "${f%.*}" "$FIXTURES/$f" || rc=1
  done
  return "$rc"
}

# A scratch tree for a mode to materialize into. Not a command substitution:
# the trap has to be set in THIS shell or the directory is deleted the moment
# the subshell returns it.
scratch() {
  SCAN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/leak-scan.XXXXXXXX")"
  trap 'rm -rf "$SCAN_DIR"' EXIT
}

# The whole tracked tree, read from the INDEX: `git checkout-index`
# materializes it into the scratch tree, so "what the gate scanned" and "what
# the commit contains" are the same bytes.
scan_tree() {
  local files=() f
  while IFS= read -r f; do files+=("$f"); done < <(git ls-files)
  if [ "${#files[@]}" -eq 0 ]; then
    echo "leak-scan: enumerated 0 tracked files — the scan is broken, not the tree." >&2
    exit 1
  fi
  scratch
  git checkout-index --all --force --prefix="$SCAN_DIR/"
  cd "$SCAN_DIR"
  scan_set "${files[@]}" || exit 1
  echo "leak-scan: ${#files[@]} tracked files, no disclosure findings"
}

# What one commit publishes: the blobs it adds or rewrites, plus its MESSAGE.
# Blobs out of the commit, never the index or the worktree — in a checkout many
# agents share, both of those carry other people's in-flight text, and a gate
# must judge the author for what the author wrote. The message is scanned
# because it is published prose that lands in no file at all: a `-m` note is
# the whole of what `bl close` writes, and AGENTS.md governs it like a body.
scan_commit() {
  local rev="$1" files=() f rc=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    files+=("$f")
  done < <(git diff-tree --no-commit-id --name-only -r -m --root \
    --diff-filter=ACMR "$rev" | sort -u)
  scratch
  mkdir "$SCAN_DIR/tree"
  # No blobs is not a broken scan here, unlike the tree mode: a commit that
  # only deletes (an archived ball) publishes its message and nothing else.
  if [ "${#files[@]}" -gt 0 ]; then
    # `-m`: take the bytes, not the archive's timestamps — a clock skewed
    # against the commit's date makes tar warn on stderr, and this scan's
    # stderr is a plugin's user-facing channel.
    git archive "$rev" -- "${files[@]}" | tar -xm -C "$SCAN_DIR/tree"
  fi
  git log -1 --format=%B "$rev" >"$SCAN_DIR/message"
  cd "$SCAN_DIR"
  scan message || rc=1
  (cd tree && scan_set ${files[@]+"${files[@]}"}) || rc=1
  [ "$rc" -eq 0 ] || exit 1
  echo "leak-scan: ${#files[@]} file(s) and the message of $rev, no disclosure findings"
}
