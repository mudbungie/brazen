+++
title = "claude-code provider: subprocess pass-through via the installed claude CLI"
created = 1784698669
updated = 1784698724
claimant = "Prostheses-b0b6"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
## Why
A provider row that drives the locally installed Claude Code CLI (claude, v2.1.217 here) as a pure model pass-through. Two wins: (1) an anthropic-model path that needs NO API key — claude carries its own OAuth credential; (2) lernie's make smoke gains a zero-cost-config anthropic-family target (its SMOKE_PROVIDER override can then name this row).

## Shape (design to be settled by the spec, per this repo's design-first rule)
This is NOT just a config row: every existing protocol rides HTTP (base_url + transport.rs). Claude Code is a subprocess — so the capability is (a) an exec-style transport seam alongside HTTP, and (b) a protocol impl mapping claude's --print stream-json event dialect to canonical events. Spec first (specs/, living document, edited like code), wired into the architecture.md registry pattern; implementation second.

## Pass-through requirements (the user's ask: suppress ALL native behaviors/context)
One request in, one model response out — no agentic loop, no tools, no repo context, no session state. Candidate flags (verify against claude --help; pin exact set in the spec): -p/--print with --output-format stream-json, --bare (skip hooks/LSP/plugins), --system-prompt from the canonical request (+ --exclude-dynamic-system-prompt-sections), --disallowedTools for the built-ins, --max-turns 1, no session persistence, --model from the request, --include-partial-messages for streaming deltas. Settle in the spec: model discovery (--list-models story for a CLI-backed row), thinking-block mapping, exit-code/timeout semantics, how auth="none" interacts with claude's own logged-out state (a crisp canonical error, not a hang — the CLI must never dangle interactively under bz).

## Constraints
Repo conventions bind: spec in specs/ first, 300-line cap on .rs, 100% line coverage, make check green, no AI credit in commits. The canonical-protocol and architecture specs are the frame; extend registries, don't fork paths.