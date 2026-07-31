+++
title = "live reasoning-summary fuzz case is RED — object-shaped reasoning is rejected at parse"
created = 1785200955
updated = 1785474344
claimant = "Sulfates-1ad0"
priority = 3
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["bug", "testing"]
+++
The one live case named for the reasoning summary channel cannot pass. Found while
closing bl-f90e (the encoder never requested `reasoning.summary`); this case was
supposed to be the thing that caught that, and it proved nothing.

## The defect

`tests/live_fuzz_openai.rs:119-124` builds the case body:

    let mut reason = valid();
    reason.insert(
        "reasoning".into(),
        json!({ "effort": "high", "summary": "detailed" }),
    );

registered at `:141` as `("reasoning-summary", Shape::Reasoning, body(&reason))`.

`body()` is piped to stdin as a CANONICAL request and `args()`
(`tests/live_support/openai.rs:43`) carries no `--raw` — so it goes through the
canonical parser, where `reasoning` is a typed `Option<ReasoningEffort>` accepting
only the `low|medium|high` string. An object is a parse error:

    $ bz --provider codex --json < r.json ; echo "exit: $?"
    {"type":"error","kind":"parse_input","message":"malformed canonical request:
     unknown variant `effort`, expected one of `low`, `medium`, `high` at line 1 column 109"}
    exit: 64

`check_accept` (`live_support/openai.rs:118`) requires exit 0, so the case fails
before a byte reaches the network. Verified 2026-07-26 against installed bz.

## Why nobody noticed

It is TRIPLE gated: `#[ignore]`, plus `BRAZEN_LIVE=1`, plus a second spend opt-in
`BRAZEN_LIVE_FUZZ_SPEND=1` (header, `:10-17`). Nothing in `.github/workflows/ci.yml`
runs `--ignored` or sets either variable — grep for `BRAZEN_LIVE|--ignored|fuzz` over
the workflows returns NOTHING. `make check` does not reach it either. So the suite has
no automatic runner at all; see the release-gate ball this one blocks.

The stale claims to correct while here: `live_support/openai.rs:100-102` says
"verified live 2026-06-17, bl-f308 — the bl-0272 guess held, no decoder gap", and
`live_fuzz_openai.rs:118` says the riddle "reliably triggers one (3/3 live)". Whatever
was true then, the case as committed cannot run today.

## Fix

Now that bl-f90e (c8f01dd) makes `encode` request `{"effort": …, "summary": "auto"}`
unconditionally, the hand-written wire object is not just broken but unnecessary — the
typed knob reaches the summary channel by itself:

    reason.insert("reasoning".into(), json!("high"));

That also makes the case exercise the REAL encoder projection rather than asserting a
decoder against a body the encoder would never emit — which is precisely the blind spot
that let bl-f90e ship. `Shape::Reasoning` (`live_support/openai.rs:103-108`) already
asserts the right grammar (`thinking` content_start + `thinking_delta`, then text) and
needs no change.

Keep the riddle prompt and high effort: the summary is MODEL DISCRETION and a trivial
prompt can return an empty one (observed live 2026-07-26 — one run of three printed no
thinking at all). If the case proves flaky even so, that is a property of the channel,
not a regression — record it rather than weakening the assertion.

## Scope

`tests/live_fuzz_openai.rs` and `tests/live_support/openai.rs` only. Disjoint from the
other balls filed 2026-07-26.