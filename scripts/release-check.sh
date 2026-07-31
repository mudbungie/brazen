#!/usr/bin/env bash
# The release gate runner (bl-3dc4) — the executable form of specs/release.md §3:
# "the release gate is a human-run `make release-check` on a credentialed
# workstation, and it runs on main's tip immediately before the release PR is
# merged". Entry point is `make release-check`; the Makefile stays the single
# source of truth for what each target does — this script only SEQUENCES the
# ladder's release rung and renders the roster. The spec owns the why/when.
#
# What it runs: the offline gate (`make check`) plus every live suite that runs
# UNATTENDED from one command (§2). `tests/oauth_smoke.rs` is deliberately absent —
# it opens a browser for consent, so it is off the ladder entirely (§2) and is a
# change-triggered manual check instead.
#
# It sets NO environment. Every suite already names its own gate in its module doc
# (BRAZEN_LIVE, BRAZEN_LIVE_FUZZ_SPEND, OLLAMA_SMOKE, the per-provider key vars),
# self-gates on it, and prints its own skip reason: "the release rung adds no new
# gate spellings" (§2). This script matches those printed words; it never re-spells
# a gate, so a suite's gate can move without touching this file.
#
# What it prints: the ROSTER §5 requires in the release PR comment — what ran, what
# skipped, and why — built from the suites' own output lines, verbatim.
#
# Exit is non-zero when:
#   * a suite FAILED (§4: every live assertion is deterministic unless the suite
#     declares the case model-discretion, so an undeclared failure is BLOCKING), or
#   * nothing was exercised (§5: "zero providers exercised is a FAILED release gate,
#     not a clean no-op") — a credential-less box can never print a green gate.
# A suite that self-gated off is a SKIP, never a pass (§4: a check that never ran
# told us nothing about brazen).
set -u

cd "$(dirname "$0")/.."
MAKE="${MAKE:-make}"
logs="$(mktemp -d)"
trap 'rm -rf "$logs"' EXIT

# The live rung: label | command. Cheapest and safest first — the free/near-free
# suites, then the token-costing ones, with the OAuth circuit LAST because its
# silent-refresh case rotates the real stored refresh token (auth.md §10.3).
suites=(
  "live_conformance|cargo test --test live_conformance -- --ignored --nocapture"
  "ollama_smoke|cargo test --test ollama_smoke -- --ignored --nocapture"
  "smoke.sh|$MAKE smoke"
  "live_fuzz_openai|cargo test --test live_fuzz_openai -- --ignored --nocapture"
  "live_encode_openai|cargo test --test live_encode_openai -- --ignored --nocapture"
  "live_oauth_openai|cargo test --test live_oauth_openai -- --ignored --nocapture"
)

# A suite that never reached a provider says so and exits 0 — these are the suites'
# OWN words: the env/cred gates ("skipping …"), a whole spend-gated body ("SKIPPED
# all …"), conformance's credential-less no-op ("0/7 providers exercised"), and
# smoke.sh with no key present ("0 passed, …").
skipped_re='^skipping |SKIPPED all |^0/[0-9]+ providers exercised|^0 passed,'
# The roster lines worth quoting back: per-provider RUN/SKIP, suite headers, the
# smoke PASS/FAIL/SKIP rows, and the two count lines.
evidence_re='^skipping |^== |^PASS  |^SKIP  |^FAIL  |^[0-9]+/[0-9]+ providers exercised|^[0-9]+ passed,|SKIPPED'

# run LABEL COMMAND LOGFILE — run one rung, echo it live, keep its log, and set the
# global `status` to PASS / SKIP / FAIL. Non-zero exit is FAIL; exit 0 while the log
# carries one of the suites' own "never reached a provider" lines is SKIP.
run() {
  printf '\n==> %s  (%s)\n' "$1" "$2"
  eval "$2" 2>&1 | tee "$3"
  code="${PIPESTATUS[0]}"
  if [ "$code" -ne 0 ]; then
    status=FAIL
  elif grep -qE "$skipped_re" "$3"; then
    status=SKIP
  else
    status=PASS
  fi
}

# 1. The offline gate. A hard stop: a broken tree must not spend tokens, and every
# rung below the release rung still holds at release time (§3).
run "offline gate (make check)" "$MAKE check" "$logs/offline.log"
offline="$status"
results=("$offline|offline gate (make check)|$MAKE check")
if [ "$offline" != PASS ]; then
  printf '\nrelease gate ABORTED: the offline gate is red — fix the tree before spending.\n' >&2
  exit 1
fi

# 2. The live rung.
ran=0 skipped=0 failed=0 i=0
for entry in "${suites[@]}"; do
  label="${entry%%|*}"
  i=$((i + 1))
  run "$label" "${entry#*|}" "$logs/$i.log"
  results+=("$status|$label|${entry#*|}")
  case "$status" in
    PASS) ran=$((ran + 1)) ;;
    SKIP) skipped=$((skipped + 1)) ;;
    FAIL) failed=$((failed + 1)) ;;
  esac
done

# 3. The roster (§5) — the record that goes in the release PR comment (§3).
rule() { printf '%s\n' "----------------------------------------------------------------------"; }
printf '\n'
rule
echo "RELEASE GATE ROSTER — specs/release.md §5 — paste into the release PR"
echo "tree:  $(git rev-parse --short HEAD) on $(git rev-parse --abbrev-ref HEAD),  $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
rule
i=0
for r in "${results[@]}"; do
  IFS='|' read -r st lbl _cmd <<<"$r"
  printf '%-4s  %s\n' "$st" "$lbl"
  log="$logs/$i.log"
  [ "$i" -eq 0 ] && log="$logs/offline.log"
  # `uniq`: a suite with several test fns prints its gate reason once per fn.
  grep -hE "$evidence_re" "$log" 2>/dev/null | uniq | sed 's/^/        /'
  i=$((i + 1))
done
rule
printf 'live suites: %d exercised a provider, %d skipped, %d failed\n' "$ran" "$skipped" "$failed"

# §5 ties skip tolerance to an existing explicit signal — the diff. A provider whose
# row or protocol code moved in the release window must have RUN; this prints the
# input to that judgement, it does not make it.
prev="$(git describe --tags --abbrev=0 2>/dev/null || true)"
if [ -n "$prev" ]; then
  touched="$(git diff --name-only "$prev..HEAD" -- src/protocol data/defaults.toml tests/live_conformance.rs)"
  printf 'release window %s..HEAD touched provider code:\n%s\n' "$prev" \
    "$([ -n "$touched" ] && echo "$touched" | sed 's/^/        /' || echo '        (none)')"
fi

# 4. The verdict.
verdict=0
if [ "$failed" -gt 0 ]; then
  verdict=1
  echo "RESULT: FAILED — $failed blocking suite failure(s) (§4: an undeclared assertion"
  echo "        failure is a defect in brazen or a real dialect change; resolve it in the"
  echo "        tree before the version ships). If a FAILED case is one the suite DECLARES"
  echo "        model-discretion, §4 allows up to 3 re-runs; passing any run is green, and"
  echo "        failing all three ships only with a signoff in the release PR comment naming"
  echo "        the case, provider and model, plus a filed bl ball. Re-run a suite with:"
  for r in "${results[@]}"; do
    IFS='|' read -r st _lbl cmd <<<"$r"
    [ "$st" = FAIL ] && printf '          %s\n' "$cmd"
  done
fi
if [ "$ran" -eq 0 ]; then
  verdict=1
  echo "RESULT: FAILED — zero providers exercised (§5: a credential-less run is a FAILED"
  echo "        gate, not a clean no-op — \"green\" would mean \"nothing was asked\")."
fi
if [ "$verdict" -eq 0 ]; then
  echo "RESULT: PASS — the version may be published. Skipped providers ship NAMED, not"
  echo "        assumed: confirm above that every provider touched in the release window ran."
fi
rule
exit "$verdict"
