+++
title = "codex backend DROPPED its stream:true mandate — accepts stream:false (200) where bl-b72f asserted a 400; verify the other mandates haven't drifted too"
created = 1781682290
updated = 1781723940
claimant = "Sheikdoms"
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
Decide whether to also drop `store:false`/instructions assertions if THEY drift, and whether the codex base now tolerates streaming as merely optional. Low urgency — the suite is already corrected; this is the record + a prompt to re-audit the remaining mandates periodically.## Resolution (bl-cc84)
Two decisions, both NO-new-mechanism:

1. **Don't pre-emptively weaken `missing-store`/`missing-instructions`.** Both STILL 400 (re-verified live 2026-06-17). The error-matrix assertion IS the drift detector — keep it asserting the 400 + surfaced wording. The codified policy: if either row later starts returning 200, MOVE it to the acceptance set (assert exit 0 + canonical grammar, the `stream:false` precedent), NOT delete it — so the suite keeps guarding against a silent re-imposition. This is now a comment in `live_fuzz_openai.rs`'s error matrix ("DRIFT POLICY") so the next auditor reclassifies rather than panics/deletes.

2. **Streaming is now OPTIONAL (not "stream:false replaces stream:true").** Both spellings return 200 and both are guarded: `stream:true` rides `valid()` (every other acceptance case), `stream:false` is its own acceptance case. No new test needed; noted in the `stream-false` case comment.

Re-audit prompt: the remaining mandates are live tripwires, not assumptions — a future fuzz run that fails on `missing-store`/`missing-instructions` IS the signal to reclassify (per the policy comment). No periodic manual sweep required beyond running the existing spend-gated suite.

Deliverable: comment-only edits to `bz/tests/live_fuzz_openai.rs` (drift policy + streaming-optional note). Suite behavior already corrected in bl-f8f7; memory note already updated there.