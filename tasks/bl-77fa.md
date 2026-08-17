+++
title = "bz --login is unreachable on a default install: ship the oauth2 row (auth §10.5's deferred decision, now ruled)"
created = 1786937283
updated = 1786937283
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["ergonomics", "auth"]
+++
On a default install `bz --login` is unreachable: `data/defaults.toml` carries no
row with `auth = "oauth2"`, and `run_login`/`resolve_oauth` refuse a provider whose
resolved row has no `[provider.oauth]` block (exit 78). So a first-time user's only
sign-in path is hand-authoring an API key into a config file. The downstream
consumer (a GUI that shells `bz`) reports this as the blocking gap: browser sign-in
is a documented recipe, never a shipped capability.

Operator ruling 2026-08-16: browser login SHOULD ship by default. Not supporting a
browser login out of the box is obnoxious. That decides the question `specs/auth.md`
§10.5 deliberately deferred:

> **Decision deferred to the operator — ship this row built-in, or keep it a
> recipe?** §7 states "**no built-in OAuth row ships for v0.1**" (vendor policy =
> operator data). The Codex `client_id` is a *published public* client, so shipping
> a built-in `openai-chatgpt` row in `defaults.toml` would be defensible UX. But it
> would reverse §7's deliberate stance and bake one vendor's login policy into the
> binary. **Recommendation: keep it a documented recipe** (README + this §10.5
> block) the operator pastes into their config — preserving "the core never ships
> vendor OAuth policy" — unless we consciously revise §7. This is a one-line
> decision, not a design fork; flagged, not silently chosen.

It is now consciously chosen the other way.

## What ships

Move the §10.5 `openai-chatgpt` row from README recipe to `data/defaults.toml`
verbatim — the same row, no new fields, no new Rust. Every fact in it is already
committed text (spec §10.5 + README), and every one was live-validated end to end
(§10.7): the public Codex `client_id`, the `auth.openai.com` endpoints, the
`localhost:1455/auth/callback` registered redirect, the three authorize params, the
`ChatGPT-Account-ID` header, `store:false`, the three `unsupported_body_keys`, and
the `[provider.models]` discovery override. Shipping it is a data move, which is the
whole point of the vendor-blind design — the core still compiles in no login policy.

Placed LAST in the table and claiming NO `model_prefixes`, so nothing about routing
or the zero-config default moves: `anthropic` stays the head row a bare `bz "q"`
reaches, and `openai-chatgpt` stays opt-in via explicit `--provider`, exactly like
its sibling `openai-responses`.

## What does NOT change

The Anthropic subscription-OAuth stance is untouched. bl-a661's conservative reading
stands — third-party use of a subscription OAuth token is restricted by that
vendor's terms, so no `anthropic-oauth` row ships and no turnkey recipe for it is
published. The existing guard test is not deleted, it is narrowed: it stops asserting
"no oauth2 row exists at all" and starts asserting "no Anthropic OAuth row exists",
which is the invariant that ruling actually bought.

`bz --login` keeps the device flow as its default and `--browser` as the opt-in
loopback flow (auth §7: selected by capability, not vendor — device works over SSH).
This ball adds no flag and no verb.

## After

    bz --login --provider openai-chatgpt --browser

works on a fresh install with no config file at all.

## Deliverables

- `data/defaults.toml` — the row.
- `src/tests/oauth2_provider_recipe.rs` — invert the guard; keep the general
  mechanism tests.
- The two ordered-name enumerations (`list_providers`, `config_priority`) and the
  row count.
- New tests: the shipped row resolves with every §10.5 field intact; routing and the
  zero-config head row are unmoved; `--login --browser` against it builds the
  registered redirect and authorize URL.
- `specs/auth.md` §7 + §10.5, `specs/architecture.md` §13 item 3, `README.md` — the
  recipe becomes a "ships by default" note; the deferred-decision block records the
  ruling rather than the question.