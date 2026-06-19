+++
title = "--raw path omits Content-Type: application/json — JSON-body providers misparse the verbatim body (openai chat/completions 400 you-must-provide-a-model-parameter)"
created = 1781830146
updated = 1781830146
priority = 2
tags = ["bug"]
+++
## Defect

`--raw` sends the stdin body verbatim but sets NO `Content-Type` header. The `Input::Raw` arm in `src/run/serve.rs` (~line 110) builds `WireRequest::new(format!("{}{}", ctx.base_url, proto.path(&ctx)), bytes)` with an empty header vec; `auth.apply` then adds ONLY auth headers. Every protocol's `encode()` sets `content-type: application/json` (e.g. src/protocol/openai/encode/mod.rs:60; anthropic/mistral/responses likewise) — but `--raw` BYPASSES encode, so that header is never set.

## Live repro (operator OpenAI key, redacted)

```
printf '{"model":"gpt-4o-mini","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"say ok"}]}' \
  | bz --provider openai --model gpt-4o-mini --api-key <KEY> --raw
```

Observed: provider returns HTTP 400 `{"error":{"message":"you must provide a model parameter","type":"invalid_request_error"}}` — the model IS in the body, but OpenAI cannot parse a body with no Content-Type as JSON.

Proven via curl (bypassing bz):
- curl WITHOUT `-H 'Content-Type: application/json'` on the SAME body -> HTTP 400, identical "you must provide a model parameter" body.
- curl WITH the header -> HTTP 429 insufficient_quota (the account's real state).

So the missing header is the cause, byte-for-byte.

## Scope

Surfaced by the standard openai row's live `--raw` smoke probe (FAIL openai raw exit 69, 166 bytes). The smoke's raw_body for anthropic/openai/mistral/openai-responses all build JSON; all four ride this path. openai-responses raw happened to PASS live (that endpoint tolerates the missing header / defaults to JSON) but openai chat/completions does NOT. Latent since `--raw` shipped; never caught because no live standard key drove the openai raw probe to a real provider before.

## Expected

The `--raw` path should carry `content-type: application/json` for JSON-body protocols (or default the header when none is present), so a verbatim JSON body is parsed by the provider.

Severability note: content-type is a PROTOCOL/dialect fact. A `Protocol::content_type()` (DATA, like `path()`/`models_path()`) read by BOTH `encode` and the raw arm would be the single-source fix — encode stops hardcoding the string, raw inherits it, one home. (anthropic/openai/mistral/responses/ollama = application/json; google streaming endpoint also JSON; verify each.)

## Close gate

make check (fmt + clippy -D + linecount + 100% cov). Add a fixture/unit test pinning that the raw WireRequest carries content-type for a JSON protocol, and ideally re-run scripts/smoke.sh against a quota'd key to confirm PASS openai raw.