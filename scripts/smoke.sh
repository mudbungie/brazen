#!/usr/bin/env bash
# Live smoke harness (bl-aba5): one tiny real request per provider, asserting a
# clean event stream + exit 0 — the executable form of the README's "smoke-tested
# live" note. The hand-authored golden fixtures for google/ollama/responses/mistral
# were validated shape-by-shape against authoritative provider specs; this harness
# is how you reconfirm against a *live* endpoint when a key is in hand.
#
# Not part of `make check` (it needs real keys + network — neither belongs in the
# pure-core coverage gate). A provider whose key env-var is absent is SKIPPED, not
# failed, so partial credentials still exercise what they can. Exit is non-zero
# only when a provider that DID run came back dirty.
#
#   make smoke                 # build, then probe every provider with a key present
#   OPENAI_API_KEY=… make smoke
#   BZ=target/release/bz scripts/smoke.sh   # reuse a prebuilt binary
set -u

BZ="${BZ:-cargo run -q -p bz --}"
PROMPT="Reply with exactly the word: ok"

pass=0 fail=0 skip=0

# provider | key env-var (empty = no auth) | model | probe host:port (keyless only)
rows=(
  "anthropic|ANTHROPIC_API_KEY|claude-haiku-4-5-20251001|"
  "openai|OPENAI_API_KEY|gpt-4o-mini|"
  "openai-responses|OPENAI_API_KEY|gpt-4o-mini|"
  "mistral|MISTRAL_API_KEY|mistral-small-latest|"
  "google|GEMINI_API_KEY|gemini-1.5-flash|"
  "ollama||llama3.2|localhost:11434"
)

for row in "${rows[@]}"; do
  IFS='|' read -r provider keyvar model probe <<<"$row"

  args=(--provider "$provider" --model "$model" --max-tokens 16 --stream)
  if [ -n "$keyvar" ]; then
    key="${!keyvar:-}"
    # GEMINI_API_KEY or the GOOGLE_API_KEY alias both feed the google row.
    [ -z "$key" ] && [ "$keyvar" = "GEMINI_API_KEY" ] && key="${GOOGLE_API_KEY:-}"
    if [ -z "$key" ]; then
      printf 'SKIP  %-18s (%s unset)\n' "$provider" "$keyvar"
      skip=$((skip + 1))
      continue
    fi
    args+=(--api-key "$key")
  elif [ -n "$probe" ] && ! (exec 3<>"/dev/tcp/${probe/:/\/}") 2>/dev/null; then
    printf 'SKIP  %-18s (%s unreachable)\n' "$provider" "$probe"
    skip=$((skip + 1))
    continue
  fi

  out="$($BZ "${args[@]}" "$PROMPT" 2>/dev/null)"
  code=$?
  if [ "$code" -eq 0 ] && [ -n "$out" ]; then
    printf 'PASS  %-18s exit 0, %d bytes streamed\n' "$provider" "${#out}"
    pass=$((pass + 1))
  else
    printf 'FAIL  %-18s exit %d, %d bytes\n' "$provider" "$code" "${#out}"
    fail=$((fail + 1))
  fi
done

printf '\n%d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
