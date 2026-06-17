#!/usr/bin/env bash
# Live smoke harness (bl-aba5): tiny real requests per provider — a happy probe
# (good key → clean event stream + exit 0) and a bad-key error probe (bl-e99e →
# correct non-zero exit + a non-empty surfaced provider error body) — the
# executable form of the README's "smoke-tested live" note. The hand-authored
# golden fixtures for google/ollama/responses/mistral were validated shape-by-shape
# against authoritative provider specs; this harness is how you reconfirm against a
# *live* endpoint when a key is in hand.
#
# Both request channels (§5.5) are exercised on the happy path per provider (bl-1d07):
# the positional prompt (argv → one User message) AND a canonical request piped on
# stdin (the read_request→parse path). The stdin request carries only `messages`;
# --model and the gen-params fill the rest via fill_absent, so a clean stream proves
# the whole parse→fill→encode→stream chain end to end, not just the argv constructor.
#
# Both OUTPUT modes are asserted too (bl-0ab8): the default text sink only checks
# non-empty bytes, which a decode/projection regression slips past. So `--json`
# asserts the canonical NDJSON contract (§5.2 — a `MessageStart`(v=1)…`End` envelope)
# and `--raw` asserts verbatim passthrough: provider-native bytes carrying NONE of the
# framing brazen would inject (§5.4 — no appended `end`). --raw is fed a provider-native
# request on stdin (its symmetric input, §5.4): the positional prompt is ignored under
# --raw, so a black-box passthrough test must speak the wire dialect itself — the lone
# place this harness duplicates encode's request shape, on purpose.
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
# A key no provider will honor — drives the bad-key error path (bl-e99e).
BADKEY="bz-smoke-deliberately-invalid-key"

# The provider-native streaming request for the --raw channel (§5.4): raw skips
# encode, so the body must already be wire-shaped. Keyed on the protocol family —
# anthropic/openai/mistral share the chat shape; responses, google, and ollama each
# differ. $1 = provider, $2 = model. Google streams via the URL verb, so its body
# carries no stream flag; every other dialect sets `"stream":true` for SSE/NDJSON.
raw_body() {
  case "$1" in
    anthropic | openai | mistral)
      printf '{"model":"%s","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"%s"}]}' "$2" "$PROMPT" ;;
    openai-responses)
      printf '{"model":"%s","max_output_tokens":16,"stream":true,"input":"%s"}' "$2" "$PROMPT" ;;
    google)
      printf '{"contents":[{"parts":[{"text":"%s"}]}],"generationConfig":{"maxOutputTokens":16}}' "$PROMPT" ;;
    ollama)
      printf '{"model":"%s","messages":[{"role":"user","content":"%s"}],"stream":true}' "$2" "$PROMPT" ;;
  esac
}

pass=0 fail=0 skip=0

# Run one bz invocation and assert the contract for its output MODE. Args:
#   $1 label   — display tag (argv/stdin/json/raw)
#   $2 mode    — which contract to assert: text | json | raw
#   $3 payload — stdin piped to bz ("" → no stdin; the prompt rides argv)
#   $4… argv   — bz flags (the mode's own --json/--raw, if any, is part of this)
# `provider` is read from the enclosing loop.
probe() {
  local label="$1" mode="$2" payload="$3"; shift 3
  local out code ok=1 detail
  if [ -n "$payload" ]; then
    out="$(printf '%s' "$payload" | $BZ "$@" 2>/dev/null)"; code=$?
  else
    out="$($BZ "$@" </dev/null 2>/dev/null)"; code=$?
  fi
  case "$mode" in
    # Default sink: a clean stream is exit 0 + any bytes (text projection drops framing).
    text)
      { [ "$code" -eq 0 ] && [ -n "$out" ]; } && ok=0
      detail="exit $code, ${#out} bytes" ;;
    # Canonical NDJSON (§5.2): first line MessageStart stamped v=1, last line the End token.
    json)
      local first last
      first="$(printf '%s\n' "$out" | head -n1)"
      last="$(printf '%s\n' "$out" | grep -v '^$' | tail -n1)"
      { [ "$code" -eq 0 ] \
        && [[ "$first" == '{"type":"message_start","v":1,'* ]] \
        && [ "$last" = '{"type":"end"}' ]; } && ok=0
      detail="exit $code, $(printf '%s\n' "$out" | grep -c .) events" ;;
    # Passthrough (§5.4): verbatim provider bytes — non-empty, exit 0, and carrying
    # NONE of brazen's framing. The discriminator is the `v:` schema field on
    # message_start (brazen's invention; no provider emits it — Anthropic's native SSE
    # *does* carry "type":"message_start", so the bare type can't tell them apart) plus
    # the canonical End token brazen explicitly never appends to a raw stream.
    raw)
      { [ "$code" -eq 0 ] && [ -n "$out" ] \
        && ! printf '%s' "$out" | grep -qF '"type":"message_start","v":' \
        && ! printf '%s' "$out" | grep -qF '{"type":"end"}'; } && ok=0
      detail="exit $code, ${#out} bytes verbatim" ;;
  esac
  if [ "$ok" -eq 0 ]; then
    printf 'PASS  %-18s %-5s %s\n' "$provider" "$label" "$detail"
    pass=$((pass + 1))
  else
    printf 'FAIL  %-18s %-5s %s\n' "$provider" "$label" "$detail"
    fail=$((fail + 1))
  fi
}

# Error path (bl-e99e): a deliberately-bad key must yield a NON-ZERO exit (auth 77,
# or the provider-error mapping — google answers a bad key with 400→69) AND a
# non-empty surfaced provider error on STDERR (bl-5fe6 carries the upstream non-2xx
# body; text mode shows the error `message`). Argv channel only; auth-specific, so
# keyless providers never call it. `2>&1 >/dev/null` captures stderr, drops stdout.
probe_error() {
  local err code
  err="$($BZ "$@" </dev/null 2>&1 >/dev/null)"; code=$?
  if [ "$code" -ne 0 ] && [ -n "$err" ]; then
    printf 'PASS  %-18s %-5s exit %d, %d-byte provider error\n' "$provider" "error" "$code" "${#err}"
    pass=$((pass + 1))
  else
    printf 'FAIL  %-18s %-5s exit %d, %d-byte error (want non-zero + body)\n' "$provider" "error" "$code" "${#err}"
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
    errargs=("${args[@]}" --api-key "$BADKEY")  # base + bad key, for the error probe
    args+=(--api-key "$key")
  elif [ -n "$probe" ] && ! (exec 3<>"/dev/tcp/${probe/:/\/}") 2>/dev/null; then
    printf 'SKIP  %-18s (%s unreachable)\n' "$provider" "$probe"
    skip=$((skip + 1))
    continue
  fi

  # Two input channels (argv/stdin, default text sink) × two output-mode contracts
  # (json/raw). --raw ignores the positional prompt and reads its wire-native body
  # from stdin; the extra encode flags in `args` are inert there (no encode runs).
  probe argv text "" "${args[@]}" "$PROMPT"
  probe stdin text "$REQUEST" "${args[@]}"
  probe json json "" "${args[@]}" --json "$PROMPT"
  probe raw raw "$(raw_body "$provider" "$model")" "${args[@]}" --raw
  # Error path (bl-e99e): a bad key → non-zero exit + surfaced provider error. Auth-
  # specific, so keyless providers (no errargs) never run it.
  [ -n "$keyvar" ] && probe_error "${errargs[@]}" "$PROMPT"
done

printf '\n%d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
