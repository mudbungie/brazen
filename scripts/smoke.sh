#!/usr/bin/env bash
# Live smoke harness (bl-aba5): one tiny real request per provider, asserting a
# clean event stream + exit 0 — the executable form of the README's "smoke-tested
# live" note. The hand-authored golden fixtures for google/ollama/responses/mistral
# were validated shape-by-shape against authoritative provider specs; this harness
# is how you reconfirm against a *live* endpoint when a key is in hand.
#
# Both request channels (§5.5) are exercised per provider (bl-1d07): the positional
# prompt (argv → one User message) AND a canonical request piped on stdin (the
# read_request→parse path). The stdin request carries only `messages`; --model and
# the gen-params fill the rest via fill_absent, so a clean stream proves the whole
# parse→fill→encode→stream chain end to end, not just the argv constructor.
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
# The same prompt as a minimal canonical request for the stdin channel — model and
# gen-params are left absent so the flags fill them (fill_absent). PROMPT is the one
# source; it holds no JSON metacharacters, so this naive interpolation stays valid.
REQUEST="$(printf '{"messages":[{"role":"user","content":"%s"}]}' "$PROMPT")"

pass=0 fail=0 skip=0

# Run one bz invocation over a single input channel and tally it. $1 = channel
# label; $2 = stdin payload ("" → no stdin: the argv channel); rest = argv. The
# argv channel passes "$PROMPT" as the trailing arg; the stdin channel omits it
# and pipes "$REQUEST" instead. `provider` is read from the enclosing loop.
probe() {
  local channel="$1" payload="$2"; shift 2
  local out code
  if [ -n "$payload" ]; then
    out="$(printf '%s' "$payload" | $BZ "$@" 2>/dev/null)"; code=$?
  else
    out="$($BZ "$@" </dev/null 2>/dev/null)"; code=$?
  fi
  if [ "$code" -eq 0 ] && [ -n "$out" ]; then
    printf 'PASS  %-18s %-5s exit 0, %d bytes streamed\n' "$provider" "$channel" "${#out}"
    pass=$((pass + 1))
  else
    printf 'FAIL  %-18s %-5s exit %d, %d bytes\n' "$provider" "$channel" "$code" "${#out}"
    fail=$((fail + 1))
  fi
}

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

  probe argv "" "${args[@]}" "$PROMPT"
  probe stdin "$REQUEST" "${args[@]}"
done

printf '\n%d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
