# brazen leak scan — the regression half, and the reason the gate cannot rot.
#
# SOURCED, never executed: `scripts/leak-scan.sh --self-test` sources this into
# its own shell, so the harness runs against the SAME `scan`/`scan_rule`/
# `scan_paths`/`scan_binary` the gate runs — a self-test that re-implemented the
# mechanism would prove only that the copy still works. It lives in its own file
# because the scanner is near the 300-line cap and "mechanism" and "the proof
# the mechanism still bites" are a real seam, not a shaved line (AGENTS.md).
#
# A leak gate does not die by being wrong; it dies by silently matching nothing
# after a pattern is edited, and then passing everything forever. So every rule
# owns a fixture (`scripts/leak-fixtures/<rule>.txt`) in which EVERY non-comment
# line must be flagged BY THAT RULE — line granularity, not file granularity, so
# one dead alternative inside a nine-way pattern cannot hide behind the eight
# that still work — and must carry `FIXTURE_MARKER`, because no regex can tell a
# real secret from a fabricated one and only the value can say so. The other
# direction is `clean.txt` / `clean-paths.txt`: near-misses that must NOT be
# flagged, because a gate that cries wolf on a fifth of the tree gets bypassed,
# and a bypassed gate is no gate.

# Every non-blank, non-'#' line of a rule's fixture must be flagged BY THAT
# RULE and must carry FIXTURE_MARKER; nothing in the clean fixtures may be
# flagged by anything.
fixture_lines() {
  local rule="$1" fixture="$2" hit="$3" ln fails=0 n=0 content
  while IFS= read -r ln; do
    [ -n "$ln" ] || continue
    n=$((n + 1))
    content="$(sed -n "${ln}p" "$fixture")"
    if [ "$rule" = forbidden-path ]; then
      printf '%s\n' "$hit" | grep -qF "$content" || {
        echo "self-test: [$rule] line $ln of $fixture was NOT flagged" >&2; fails=1; }
      continue
    fi
    printf '%s\n' "$hit" | grep -qE ":$ln  \[" || {
      echo "self-test: [$rule] line $ln of $fixture was NOT flagged" >&2; fails=1; }
    printf '%s' "$content" | grep -qi "$FIXTURE_MARKER" || {
      echo "self-test: [$rule] line $ln of $fixture carries no '$FIXTURE_MARKER' marker — a fixture value must be unmistakably fabricated" >&2
      fails=1; }
  done <<<"$(grep -nvE '^(#|$)' "$fixture" | cut -d: -f1)"
  [ "$n" -gt 0 ] || { echo "self-test: $fixture has no cases" >&2; fails=1; }
  return "$fails"
}

self_test() {
  local rule fixture fails=0 hit p
  for rule in "${RULES[@]}" forbidden-path; do
    fixture="$FIXTURES/$rule.txt"
    if [ ! -f "$fixture" ]; then
      echo "self-test: rule '$rule' has no fixture at $fixture" >&2; fails=1; continue
    fi
    if [ "$rule" = forbidden-path ]; then
      hit="$(grep -vE '^(#|$)' "$fixture" | while IFS= read -r p; do scan_paths "$p"; done)"
    else
      hit="$(scan_rule "$rule" "$fixture")"
    fi
    fixture_lines "$rule" "$fixture" "$hit" || fails=1
  done
  # binary-content owns bytes, not lines: its fixture cannot carry a marker or
  # be read at all, so it is capped instead. 512 bytes is far too small to
  # smuggle a dump through the one file the scanner cannot look inside.
  fixture="$FIXTURES/binary-content.bin"
  [ -n "$(scan_binary "$fixture")" ] || {
    echo "self-test: [binary-content] $fixture was NOT flagged" >&2; fails=1; }
  [ "$(wc -c <"$fixture")" -le 512 ] || {
    echo "self-test: $fixture is over the 512-byte cap on unreadable fixtures" >&2; fails=1; }
  # The other direction of the binary rule, and the one an allowlist regex could
  # never prove: an unreadable file with a tracked `<name>.provenance.md` beside
  # it PASSES, and the escape is the document, not the path. Both fixtures are
  # byte-identical but for that neighbour, so this pair fails the moment the
  # declaration stops being what is consulted.
  fixture="$FIXTURES/declared-binary.bin"
  [ -f "$fixture$BINARY_PROVENANCE" ] || {
    echo "self-test: $fixture has no provenance document beside it" >&2; fails=1; }
  [ -z "$(scan_binary "$fixture")" ] || {
    echo "self-test: a declared binary ($fixture) was flagged — the provenance escape is dead" >&2; fails=1; }
  # The false-positive direction, and the half that keeps the gate usable: a
  # near-miss for every rule, none of which may be flagged.
  if ! scan "$FIXTURES/clean.txt"; then
    echo "self-test: $FIXTURES/clean.txt was flagged above — a rule is over-broad" >&2
    fails=1
  fi
  while IFS= read -r p; do
    case "$p" in '#'*|'') continue ;; esac
    [ -z "$(scan_paths "$p")" ] || {
      echo "self-test: clean path '$p' was flagged — forbidden-path is over-broad" >&2; fails=1; }
  done <"$FIXTURES/clean-paths.txt"
  [ "$fails" -eq 0 ] || exit 1
  echo "leak-scan: self-test OK — ${#RULES[@]} content rules + forbidden-path + binary-content (both directions) all live, clean fixtures unflagged"
}
