+++
title = "openai_chat has no reasoning decode — --thinking is silently always empty"
created = 1785200974
updated = 1785200974
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["protocol"]
+++
On `openai_chat`, `--reasoning` is accepted and projected to the wire, but NOTHING can
ever come back through it. `grep -rn "reasoning" src/protocol/openai/decode/` returns
NOTHING — there is no reasoning decode on this dialect at all.

So `bz --reasoning high --thinking` against an openai_chat row spends thinking tokens
and renders nothing, with no error and no adaptation notice. That is the same SYMPTOM
the owner hit on openai_responses (bl-f90e), reached by a different cause: there the
encoder failed to request the readable channel; here there is no decode path for one.

## Why this is not simply bl-f90e again

Stock OpenAI Chat Completions returns no reasoning text — the channel does not exist on
that API, so there is nothing to request and nothing to decode. For OpenAI itself the
current behavior is arguably correct and the only defect is that it is SILENT.

The gap is third-party backends on this dialect. Several (DeepSeek and others) emit a
`reasoning_content` field on the delta alongside `content`. brazen drops it on the
floor today. `data/defaults.toml` and the `providers.md` severability story both invite
routing arbitrary openai-compatible backends here, so this is reachable in normal use,
not hypothetical.

## The design call (decide before coding)

- Is `reasoning_content` decoded to `ThinkingDelta`? It is a de-facto convention across
  openai-compatible backends, not part of any spec brazen tracks. Decoding an unknown
  extension by default may be exactly right (it is inert when absent — the empty-set
  path, not a special case) or may be scope creep into per-vendor guesswork. Rule on it.
- If yes, it is a decode-side change only; the canonical `Content::Thinking` and the
  `--thinking` sink already exist and need nothing.
- Whatever is decided, `specs/openai-chat-mapping.md` gains the row — currently the
  spec is silent on reasoning DECODE, which is why the absence reads as an oversight
  rather than a ruling.

Consider also whether the silence itself deserves surfacing: a request that sets
`reasoning` on a dialect that can never return it is a known-lossy operation, and
brazen already has a vocabulary for that (named adaptations, `lossy_overrides`). Do not
add a flag — prefer an existing explicit signal, or rule that silence is correct and
say so in the spec.

## Scope

`src/protocol/openai/decode/` and `specs/openai-chat-mapping.md`. Disjoint from the
other balls filed 2026-07-26.