+++
title = "Row-level model-discovery override: --list-models for the ChatGPT-SSO Codex backend (query param + row-specific response shape)"
created = 1782710705
updated = 1782710705
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design", "bug"]
+++
## Problem

`bz --list-models --provider <chatgpt-sso-row>` fails against the OpenAI
ChatGPT-SSO Codex backend (`https://chatgpt.com/backend-api/codex`). The
generation (data-plane) path works; only model discovery is broken. Verified
live against the real backend with a stored OAuth token (2026-06-28).

Two distinct gaps, plus a fragility:

**Gap A — request needs a query param `bz` can't send.**
The Codex `/models` route demands `?client_version=X.Y.Z`:
- `GET .../codex/models`                      -> 400 `client_version Field required`
- `GET .../codex/models?client_version=0.0.0` -> 200 `{"models":[…]}`
`bz` builds the models GET as `{base_url}{models_path()}` (`src/run/models.rs:107`)
with no mechanism to attach query params. `authorize_params` exists but is
OAuth-authorize-only; nothing reaches the models GET.

**Gap B — response shape is row-specific, but decode keys are a protocol const.**
The Codex body is `{"models":[{"slug":…}]}`. `openai_responses::decode_models`
(`src/protocol/openai_responses/mod.rs:58`) hardcodes
`decode_models(body, "data", "id", "")` — the standard OpenAI shape
`{"data":[{"id":…}]}`. So even past the 400 it would 502 "malformed models list".
Key point: the same `openai_responses` protocol serves BOTH standard OpenAI
(`data`/`id`, api-key row) AND this Codex backend (`models`/`slug`, oauth row).
So the array_key/id_key are **row** data, not a protocol constant. The decode
helper (`src/protocol/json.rs:20`) is ALREADY parameterized
`(array_key, id_key, strip)` — the keys just need to come from the row.

**Fragility (attack the design):** the list is server-side **version-gated** —
`client_version=0.0.0`/`1.0.0`/`99.0.0` -> full list; `0.36.0`/`0.50.0` -> `{"models":[]}`.
So any fixed `client_version` we pin can silently go stale (return empty). The
design must either accept this, surface an empty list honestly (not an error),
and/or document it. Decide explicitly — don't let it be an accident.

## Design direction (decided 2026-06-28, subject to the design pass)

Recognize the discovery endpoint's **path, query, and response shape are ROW
data, not protocol constants.** One optional, severable per-row block:

    [[provider]]
    name = "chatgpt"
    …
    [provider.models]                       # all keys optional
    path      = "/models"                   # default: protocol models_path()
    query     = [["client_version","0.0.0"]]# default: none
    array_key = "models"                    # default: "data"  (protocol default)
    id_key    = "slug"                      # default: "id"     (protocol default)

Severability test holds: delete `[provider.models]` -> behavior reverts to the
protocol defaults, deleting config not code. Single source of truth: the
protocol still owns its DEFAULT keys/path; the row only OVERRIDES.

Open design questions for the living doc:
- Should the array_key/id_key defaults live on the protocol (so a row omitting
  them inherits) or be required when `[provider.models]` is present? (Prefer
  protocol-default inherit — less config.)
- Is `query` general (Vec<[k,v]>) or just a `client_version` field? General is
  more honest (it's a query string) and avoids a Codex-specific name; weigh vs.
  YAGNI. Mirror `authorize_params`'s shape for consistency.
- Empty-list-on-stale-version: log a note? Leave silent (a valid empty list)?
- `--raw`/`--json` interactions: none expected (discovery predates projection),
  confirm.

## Deliverable

A design edit to the living doc `specs/model-discovery.md` (the tracked
artifact; the task records the work). THEN implement under the design, 100% line
coverage, 300-line cap, merge via worktree per AGENTS.md. Add the
`[provider.models]` block to the user's `chatgpt` row in
`~/.config/brazen/config.toml` (out of repo) as the manual verification.

## Reference data (live, 2026-06-28)

Working call:
`GET https://chatgpt.com/backend-api/codex/models?client_version=0.0.0`
Headers: `Authorization: Bearer <oauth>`, `ChatGPT-Account-ID: <acct>`, `originator: codex_cli_rs`
Returns slugs: gpt-5.6-sol, gpt-5.6-terra, gpt-5.6-luna, gpt-5.5, gpt-5.4,
gpt-5.4-mini, codex-auto-review. Each entry is a rich object keyed by `slug`
(also: context_window, supported_reasoning_levels, visibility, etc. — we only
need `slug`).