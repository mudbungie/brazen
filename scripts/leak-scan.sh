#!/usr/bin/env bash
# brazen leak scan — the disclosure half of the gate, ported from yog under
# bl-39f1 (yog bl-fd5a, reworked bl-167d/bl-1007/bl-1043; the rust-bootstrap
# template). The rest of the gate asks whether the tree is well-formed
# (fmt, clippy, the 300-line cap, 100% coverage); nothing asked whether it
# DISCLOSES something. brazen holds credentials — 0600 cred files, OAuth access
# and refresh tokens, an ambient `claude_code` recipe reading the CLI's own
# login — and it is published to crates.io, where a version cannot be recalled.
# So the material it could leak is exactly the material it handles.
#
# The rules live next door in `scripts/leak-rules.sh` and the two scopes in
# `scripts/leak-modes.sh`; this file is mechanism — HOW a file is judged, as
# against WHAT counts as a finding and WHICH files are handed over.
#
#   make leak-scan                    the gate, over the whole tracked tree
#   scripts/leak-scan.sh              the same, run directly
#   scripts/leak-scan.sh FILE...      scan exactly these files (commit-msg)
#   scripts/leak-scan.sh --commit REV scan what REV publishes: the blobs it
#                                     adds or rewrites, plus its message
#   scripts/leak-scan.sh --self-test  prove every rule still fires, and that
#                                     none fires on the clean fixture
#                                     (the harness lives in leak-selftest.sh)
#
# THE TREE IT SCANS IS THE ONE IT IS RUN IN, WHICH NEED NOT BE THIS REPO
# (bl-1043). The rule table is resolved from the SCRIPT's own directory and the
# tree from `git rev-parse` in the working directory, so the same mechanism and
# the same table judge yog's index and the balls TASK STORE — a different git
# repo entirely (`<state>/balls/clones/<enc>/tasks`, holding `tasks/*.md`),
# written by `bl`, never reached by this repo's pre-commit hook. Ball bodies
# are prose on a ref that publishes beside the source, and a second copy of the
# rules for them would drift from this one inside a week. Its callers are
# `scripts/yog-leak-gate` (the balls plugin, before the store is pushed) and
# `.github/workflows/store-scan.yml` (the published ref, after).
#
# TWO SCOPES, BECAUSE THEY ANSWER DIFFERENT QUESTIONS (bl-1007). The tree mode
# asks "does this checkout carry a finding" — the right question for a commit
# hook (the tree IS your change) and for the workflow that judges the published
# ref. It is the WRONG question for a shared, long-lived checkout written by
# many agents: run at every store op, one polluted ball body refuses every
# agent's every op — create included, so the defect about the wedge could not
# be filed — and the author who wrote it is never the one who is told. The
# `--commit` mode asks "does this OP publish a finding", which is the author's
# own text at the moment of writing, and it is what a store gate wants. The
# standing-state question is still asked, once a day, by
# `.github/workflows/store-scan.yml` over the whole ref — where a hit's remedy
# (a history rewrite) belongs anyway.
#
# THE TREE MODE READS INDEX BLOBS, NOT THE WORKTREE. That is the whole of
# bl-167d's headline: this scan used to enumerate `git ls-files` and then hand
# those PATH NAMES to grep, which opens the WORKTREE file — so a leak that was
# `git add`ed and then overwritten with a clean copy on disk was committed
# without the gate ever reading the bytes it was gating. `git checkout-index`
# materializes the index into a scratch directory and the scan reads that, so
# the bytes scanned are the bytes committed. The index rather than the diff,
# for the same reason `make line-cap` reads it: a diff-only gate is a sampling,
# not an invariant, and a file that leaked once and was never touched again
# would never be looked at again.
#
# THE REGRESSION HALF IS `--self-test`, and it is the point of this file. A
# leak gate does not die by being wrong; it dies by silently matching nothing
# after a pattern is edited, and then passing everything forever. So every
# rule owns a fixture (`scripts/leak-fixtures/<rule>.txt`) in which EVERY
# non-comment line must be flagged — line granularity, not file granularity,
# so one dead alternative inside a nine-way `vendor-token` pattern cannot hide
# behind the eight that still work — and must carry `FIXTURE_MARKER`, because
# no regex can tell a real secret from a fabricated one and only the value can
# say so. `rules-audit` (the ast-grep equivalent in the Makefile) only asserts
# its fixture DIRECTORY is flagged; this is the stronger check. The other
# direction is `clean.txt` / `clean-paths.txt`: near-misses that must NOT be
# flagged, because a gate that cries wolf on a fifth of the tree gets
# bypassed, and a bypassed gate is no gate.
#
# NOTHING IS EXEMPT FROM THE TREE SCAN ANY MORE. The scanner and its rule
# table used to be skipped for being made of the patterns; they are scanned,
# and stay clean because no pattern may match its own text (leak-rules.sh
# says how). A rule fixture is scanned by every rule EXCEPT the one it is the
# fixture of — its own rule must flag it, that is its contract — which is a
# structural exemption keyed to the file's own name, not an allowlist: adding
# a file to it means adding a RULE of that name.
#
# WHAT A COMMIT HOOK CANNOT PROMISE. This scans ONE TREE. Old commits, other
# refs, pull-request and release text, Actions logs, build artifacts and
# already-published crate versions are all outside it, and no hook can reach
# them; a gate that implied otherwise would be worse than one that says so.
# They are a release CHECKLIST instead, run by a person once per publication —
# AGENTS.md, "What a commit hook cannot promise". `cargo publish` is
# irreversible: a yanked version stays downloadable.
#
# Known limits, stated rather than implied:
#   - IPv6 is matched in full 8-group form only. The compressed `::` forms
#     cannot be told from Rust path syntax (`deadbeef::cafe`) without a false
#     positive rate that would get the whole gate disabled.
#   - A four-part version string (major.minor.patch.build) is
#     indistinguishable from an IPv4 address. If one ever lands, it goes in
#     the rule's EXCEPT list.
#   - Ordinary prose is not detectable. A pasted paragraph of somebody's
#     conversation with no speaker label and no session key reads as writing;
#     `quoted-dialogue` catches the SHAPE transcripts arrive in, which is all
#     a regex can do.
#   - A fabricated value is indistinguishable from a real one, so two rules
#     take the value's own word for it — `FIXTURE_MARKER` on a rule fixture,
#     `Fixture` inside a session id (leak-rules.sh). Both are escapes a real
#     secret cannot take without an author typing the word onto it.

set -euo pipefail

# The table travels with the SCANNER, not with the tree under scan: run in the
# task store there is no `scripts/` to source, and a copy of the rules there
# would be a second definition of what counts as a leak. Resolved BEFORE the
# `cd`, or a relative `$0` would be resolved against the wrong directory.
# `--self-test` and the fixture skip stay tree-relative, because the fixtures
# are tracked files of THIS repo and only this repo's scan has any to skip.
HERE="$(CDPATH= cd -- "$(dirname -- "$(readlink -f "$0" 2>/dev/null || echo "$0")")" && pwd)"

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

FIXTURES="scripts/leak-fixtures"

# shellcheck source=scripts/leak-rules.sh
. "$HERE/leak-rules.sh"
# The two SCOPES — the tree scan and the commit scan — and the scratch trees
# they materialize into. Sourced from beside the scanner for the same reason
# the rule table is: `$0` may be reached from any tree. See leak-modes.sh.
# shellcheck source=scripts/leak-modes.sh
. "$HERE/leak-modes.sh"

# --- mechanism -------------------------------------------------------------

# Emit one line per finding: "  path:line  [rule]  <first 12 chars of match>".
# Truncated deliberately — the finding must LOCATE the leak, never reprint it
# into a terminal, a CI log or a bug report.
report() {
  awk -F: -v rule="$1" '{
    m = substr($0, length($1) + length($2) + 3)
    if (length(m) > 12) m = substr(m, 1, 12) "..."
    printf "  %s:%s  [%s]  %s\n", $1, $2, rule, m
  }'
}

# scan_rule RULE FILE... -> findings on stdout, empty if clean.
scan_rule() {
  local rule="$1"; shift
  [ "$#" -gt 0 ] || return 0
  local hits PATTERN EXCEPT WHY
  rule_fields "$rule"
  hits="$(grep -HIonE -e "$PATTERN" -- "$@" 2>/dev/null || true)"
  [ -n "$hits" ] || return 0
  if [ -n "$EXCEPT" ]; then
    hits="$(printf '%s\n' "$hits" | grep -vE ":[0-9]+:(${EXCEPT})" || true)"
  fi
  [ -n "$hits" ] || return 0
  printf '%s\n' "$hits" | report "$rule"
}

# scan_paths PATH... -> findings for the path rule.
scan_paths() {
  local p
  for p in "$@"; do
    printf '%s\n' "$p" | grep -qE "$FORBIDDEN_PATH" && printf '  %s  [forbidden-path]\n' "$p"
  done
  return 0
}

# scan_binary FILE... -> findings for content no rule can read. A file grep
# will not read as text is not clean, it is unexamined.
#
# The one escape is a DECLARATION beside the file — `<name>$BINARY_PROVENANCE`,
# a tracked document saying what the bytes are, where they came from and what
# claim they serve — never a path on a list (leak-rules.sh says why). Presence
# is the whole test: this scan cannot read the binary and will not pretend to
# judge the prose either, so what it enforces is that a human wrote something a
# reviewer can read. Delete the provenance and the binary is refused again.
scan_binary() {
  local f
  for f in "$@"; do
    [ -s "$f" ] || continue
    grep -qI '' "$f" 2>/dev/null && continue
    [ -f "$f$BINARY_PROVENANCE" ] && continue
    printf '  %s  [binary-content]\n' "$f"
  done
  return 0
}

# scan [--skip RULE] FILE... -> 0 clean, 1 with findings printed to stderr.
scan() {
  local skip=''
  if [ "${1-}" = --skip ]; then skip="$2"; shift 2; fi
  local rule found='' out PATTERN EXCEPT WHY
  for rule in "${RULES[@]}"; do
    [ "$rule" = "$skip" ] && continue
    out="$(scan_rule "$rule" "$@")"
    rule_fields "$rule"
    [ -n "$out" ] && found+="$out"$'\n'"       $WHY"$'\n'
  done
  for rule in forbidden-path binary-content; do
    [ "$rule" = "$skip" ] && continue
    case "$rule" in
      forbidden-path) out="$(scan_paths "$@")" ;;
      *)              out="$(scan_binary "$@")" ;;
    esac
    rule_fields "$rule"
    [ -n "$out" ] && found+="$out"$'\n'"       $WHY"$'\n'
  done
  if [ -n "$found" ]; then
    echo "error: leak-scan found material that must not be committed:" >&2
    printf '%s' "$found" >&2
    return 1
  fi
  return 0
}

case "${1-}" in
  # The harness is sourced, not re-implemented: it runs against the same
  # functions above that the gate runs. See scripts/leak-selftest.sh.
  --self-test) . "$HERE/leak-selftest.sh"; self_test ;;
  --commit) scan_commit "${2:-HEAD}" ;;
  '') scan_tree ;;
  *) scan "$@" || exit 1 ;;
esac
