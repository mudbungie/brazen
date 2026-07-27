+++
title = "openai_responses never requests reasoning summary — --thinking shows nothing"
created = 1785133263
updated = 1785133263
priority = 3
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["bug", "protocol"]
+++
On `openai_responses`, `--reasoning` burns thinking tokens but `--thinking` displays
nothing. Reproduced live against the `codex` row (gpt-5.4), 2026-07-26.

## Repro

    bz --reasoning low "how many rs in strawberry"   # 37 out, vs 19 out without
    bz --reasoning low --thinking "..."              # 61 out, still prints only the answer

`--json` shows why — the reasoning block carries ONLY the encrypted channel:

    {"type":"content_start","index":0,"kind":{"thinking":{"id":"rs_0f89..."}}}
    {"type":"content_delta","index":0,"delta":{"encrypted_reasoning_delta":"gAAAAABqZveIHVB..."}}

## Cause

`src/protocol/openai_responses/encode.rs:55-59` asks for the encrypted channel and
nothing else:

    if let Some(r) = req.reasoning {
        body.insert("reasoning".into(), json!({"effort": r.as_str()}));
        body.insert("include".into(), json!(["reasoning.encrypted_content"]));

OpenAI emits the readable `response.reasoning_summary_text.delta` frames only when
the request carries `reasoning.summary`. Without it there is no summary to decode:
`decode/mod.rs:40` maps that frame to `ThinkingDelta`, and the sinks render only
`ThinkingDelta` — `encrypted_reasoning_delta` falls through to `_` and drops
(by design; that blob is replay state, not display, bl-61a9).

So the encrypted blob is requested for replay and the human channel is never asked
for. Anthropic and Google have no equivalent gap — their reasoning IS the readable
channel.

## Confirmed fix (live wire)

Adding `summary` to the request produces the summary frames:

    $ bz --raw ... {"reasoning":{"effort":"medium","summary":"auto"}, ...}
    event: response.reasoning_summary_text.delta
    data: {"delta":"**Solving basic arithmetic problem**", ...}

and end-to-end through the canonical path via a `body_defaults` override:

    body_defaults = { store = false, reasoning = { effort = "medium", summary = "auto" } }
    $ bz --thinking "I have 3 apples, buy 7 more, eat 2, give away half the remainder..."
    **Solving arithmetic remainder problem**
    4 apples left.

## The design call

That `body_defaults` route is only a workaround, and a footgunny one: providers.md §6
says "the typed `--reasoning` knob, written by `encode` *before* the `extra` fold, WINS
on a same-named key", so passing `--reasoning` replaces the whole object and takes
`summary` with it. The two knobs are mutually exclusive today.

Two candidate resolutions:

- **A (recommended, minimal).** `encode` emits `{"effort": …, "summary": "auto"}`. Zero
  interface change, no precedence change, no new flag — it just stops asking for half
  the reasoning. Open question: whether a non-OpenAI backend on this protocol 400s on
  `summary`, and if so how it opts out — `unsupported_body_keys` strips CANONICAL keys
  pre-encode (config §4.1.1) and so cannot reach a nested one. If that case is real, the
  severability answer is a row datum, not a core branch.
- **B (general).** Fold `extra` INTO the typed reasoning object per-key rather than
  losing to it wholesale. Fixes the same footgun for the Anthropic `thinking` and Google
  `thinkingConfig` escape hatches too, but it edits documented precedence in
  providers.md §6 and architecture.md §3.1 ("typed fields win"), so it is the larger door.

Whichever lands, providers.md §6 is amended in the same change — the projection table
row for `openai_responses` currently reads `"reasoning": {"effort": e.as_str()}`.

Note the summary is model-discretion regardless: a trivial prompt can return an empty
summary and print nothing. Test with a prompt that earns a summary.