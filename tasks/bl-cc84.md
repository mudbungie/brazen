+++
title = "codex backend DROPPED its stream:true mandate — accepts stream:false (200) where bl-b72f asserted a 400; verify the other mandates haven't drifted too"
created = 1781682290
updated = 1781682456
tags = ["testing", "openai"]
+++
Found while running bl-b72f's live fuzz suite as a regression check during bl-f8f7.

## The drift
bl-b72f recorded the codex backend (`openai-chatgpt`, base `https://chatgpt.com/backend-api/codex`) MANDATES `stream:true`: omitting/flipping it returned `400 {"detail":"Stream must be set to true"}`. As of 2026-06-17 that mandate is GONE — codex ACCEPTS a `stream:false` request and returns a complete (non-streamed) response. bz decodes it into the canonical event stream (message_start … content_delta … finish … end), exit 0.

## Evidence (live, gpt-5.4, by hand)
Request `{"stream":false,"store":false,"system":[…],"messages":[{"role":"user","content":[{"type":"text","text":"reply with the single word: ok"}]}]}` via `bz --provider openai-chatgpt --model gpt-5.4 --json` → exit 0, full canonical grammar, text_delta "ok". Reproduces deterministically (not the transient exit-69 flakiness).

The OTHER two mandates STILL hold (re-verified same run): missing `system` → 400 "Instructions are required"; missing `store` → 400 "Store must be set to false". `gpt-5-codex` still 400s "not supported … with a ChatGPT account".

## Done in bl-f8f7 (so the fuzz suite stays green)
`bz/tests/live_fuzz_openai.rs`: `stream-false` MOVED from the error matrix to the spend-gated acceptance set (now asserts exit 0 + canonical grammar — guards both bz's non-streaming decode AND a silent re-imposition of the mandate). Memory note bz-live-openai-chatgpt-testing updated.

## This ball
Decide whether to also drop `store:false`/instructions assertions if THEY drift, and whether the codex base now tolerates streaming as merely optional. Low urgency — the suite is already corrected; this is the record + a prompt to re-audit the remaining mandates periodically.