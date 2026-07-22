+++
title = "Support and verify an operator-owned Claude-session HTTP recipe without a built-in profile"
created = 1784699327
updated = 1784699332
claimant = "pi"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["auth", "config", "conformance"]
+++
## Goal

Enable this machine's private direct-HTTP recipe to borrow the installed Claude Code session credential and send one customized Anthropic Messages request through Brazen: caller-owned system/tools/body, one generation, no retry, no tool execution or re-entry. This is application-wire conformance only; transport-wire/TLS identity is tracked separately by bl-a0ea.

The recipe MUST remain operator-owned. Do not add a provider row to `data/defaults.toml`, a Claude-session `ProtocolId`, a public profile flag, or vendor policy to the shipped binary. Reuse `anthropic_messages` + generic row data. The shipped exec-backed `claude-code` provider (bl-b0b6) is unrelated and remains unchanged.

## Design first

Amend the living specs before implementation. Define the normalized application-wire comparison against the first inference request from Claude Code print mode: ignore secrets, random ids, header casing/order, content length, and the explicitly stripped telemetry/account/date context; pin URL/query, remaining headers, and JSON body bytes/shape owned by the private recipe. State clearly that this is not transport-wire identity.

## Generic Brazen gaps to close

1. A provider row needs an optional generation-request query (the observed Claude beta client targets `/v1/messages?beta=true`). It must append through the shared normalized/raw request tail, preserve the protocol's path as its one home, encode query data correctly, and default empty so every existing request is byte-identical.
2. Ambient OAuth credentials are borrowed foreign state. Preserve source provenance through auth resolution. A fresh borrowed credential may authenticate the request; an expired borrowed credential MUST NOT be refreshed, rotated, or copied into Brazen's store. Return Auth/77 telling the operator to refresh it with its owner. Owned credentials keep today's silent refresh/persist behavior.
3. Add offline conformance coverage from a scrubbed Claude Code v2.1.217 first-request capture and the equivalent private recipe request. The comparison must normalize only the declared volatile/stripped fields and fail on any undeclared URL/header/body drift. No real token or PII in fixtures.

## Private installation acceptance

After the generic change lands, install an operator-only provider config outside embedded defaults and a private single-shot launcher/template. It must:

- read `~/.claude/.credentials.json` as borrowed ambient OAuth;
- target the current beta Messages URL and carry the current operator-pinned Claude header bundle;
- construct the caller-owned raw Messages body (mandatory accepted Claude identity lead, custom system, custom tool array, one user turn, stream=true; telemetry/account/date reminder stripped by decision);
- use directional raw input with canonical text/NDJSON output, so Brazen does no automatic cache/body rewriting;
- stop after the first model response, surfacing `tool_use` to the caller without executing it;
- include a local fake-token/mock-server check and then one minimal live smoke call, with no credential ever printed or persisted elsewhere.

## Non-goals

- Built-in Claude-session provider/profile or changes to the exec-backed `claude-code` row.
- TLS ClientHello, ALPN, HTTP implementation fingerprint, or literal TCP bytes (bl-a0ea).
- Brazen-owned login or refresh of Claude Code credentials.
- Reproducing Claude telemetry, device/account/session metadata, dynamic account/date reminders, retries, fallback, or agent loops.