+++
title = "ollama_chat sends no options.num_ctx, and a body_defaults options object beside a typed cap is dropped whole"
created = 1786846309
updated = 1788580251
claimant = "Animations-B"
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["bug", "ollama", "encode", "config"]
+++
The `ollama_chat` encoder projects `max_tokens` onto `options.num_predict` and emits no `options.num_ctx`, so every request runs at the Ollama server's own default context rather than at anything the caller or the config can state. The model's real capacity is irrelevant to the request; a large-context local model is driven at the server default.

That alone is a narrowing (providers docs: a canonical field with no wire slot is narrowed), but the escape hatch does not compose, and that is the actual defect. `encode` inserts the typed `options` object first and folds `req.extra` with `body.entry(k).or_insert_with(...)`, a SHALLOW insert on the top-level key. So a row carrying `body_defaults = { options = { num_ctx = N } }` has that object DROPPED WHOLE and silently whenever any typed generation scalar is set — and an agent harness always sets `max_tokens`. The one config-level valve for a nested dialect field is unreachable for exactly the dialect that nests everything.

Measured against 0.0.5, driving the library with a capturing `Transport` (no server), one canonical request with `max_tokens: Some(4096)` plus one tool, against the built-in `ollama` row:

- bare row -> `"options":{"num_predict":4096}`
- row with `body_defaults = { options = { num_ctx = 32768 } }` -> `"options":{"num_predict":4096}` (the operator's value is gone, no warning)
- row with `unsupported_body_keys = ["max_tokens"]` and `body_defaults = { options = { num_ctx = 32768, num_predict = 4096 } }` -> `"options":{"num_ctx":32768,"num_predict":4096}`

The third line is the only route that works today. It is lossy and fragile as a recommendation: it requires restating the output cap inside the passthrough object, and it re-breaks the moment any other typed gen scalar is present. `temperature`/`top_p` can be cleared the same way; **`stop` cannot** — `strip_unsupported` matches `max_tokens`/`temperature`/`top_p`/`reasoning`/`output` and falls through to `req.extra.remove(other)`, which never touches the typed `req.stop`, so `unsupported_body_keys = ["stop"]` is INERT for the typed field. The built-in `claude-code` row already declares exactly that key, so this is a live second finding, not a hypothetical.

Two candidate shapes, either of which closes it:

1. Make config passthrough compose with the typed body: merge an `extra`/`body_defaults` OBJECT into a same-named object the encoder already built, one level deep, instead of dropping it. That fixes the whole class (any nested dialect field) rather than this field, and keeps "the typed field wins" per LEAF key.
2. Lift a canonical context declaration — the input-window counterpart of `max_tokens` — that each dialect projects where it has a slot (`options.num_ctx` for ollama) and narrows where it does not. This is the one that lets a caller state it per request rather than per row.

Whichever lands, three properties are the acceptance: the output cap and the context size stay DISTINCT fields; an explicit smaller operator value still wins over any default; and a size nobody stated is not fabricated. Also worth fixing beside it: make `unsupported_body_keys = ["stop"]` actually clear the typed `stop`, or say in the docs that it cannot.

Filed from yog, where the consequence was a p1: the offered local-provider route reached inference and could not produce one useful agent turn. The platform tool payload alone spent 4095 input tokens and the turn ended `finish_reason: length` after a single generated token, on a model whose own context was 262144. yog cannot fix it from its side (it authors no provider row and cannot see the server's setting) and has landed a stated caveat plus this recipe at its model picker; the caveat is pinned by a test that drives the linked brazen and fails when this changes, so it will be deleted rather than left to rot.