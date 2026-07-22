+++
title = "claude-code provider: subprocess pass-through via the installed claude CLI"
created = 1784698669
updated = 1784699128
claimant = "Prostheses-b0b6"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
## Delivered
Spec: specs/claude-code.md (linked from specs/README.md). Implementation exact to spec; make check green (fmt, clippy -D warnings, 300-line cap, 100% line coverage).

## Key decisions (spec §9)
- Invocation (verified on claude v2.1.217): `claude -p --output-format stream-json --include-partial-messages --verbose --setting-sources "" --tools "" --disable-slash-commands --strict-mcp-config --no-session-persistence --system-prompt <s|""> --model <m> [--effort low|medium|high]`, prompt on stdin. `--verbose` is REQUIRED by the CLI for -p stream-json. `--bare`/CLAUDE_CODE_SIMPLE=1 REJECTED: verified they sever claude's own OAuth ("Not logged in", exit 1). Owned residue (verified by echo-probe): one <system-reminder> (userEmail+currentDate) in the first user message; CLAUDE.md/settings/hooks/MCP/skills verified severed (canary test).
- Transport: WireRequest.exec: Option<ExecSpec> + Protocol::exec_spec (data, like path()); native spawn in src/native/exec.rs routed from HttpTransport::send; status 200 at spawn, failures in-band; stderr carried on nonzero exit; silence budget (timeouts.idle) kills a stalled child; reaped on EOF/stall/Drop. purity.rs forbids Command::new in the lib.
- Row: shipped in data/defaults.toml — name claude-code, protocol claude_code, auth none, exec = "claude" (substitutes for base_url; completed as ""), unsupported_body_keys = [max_tokens, temperature, top_p, stop, output]. No model_prefixes (opt-in via --provider, like openai-responses).
- Mapping: single-turn text-only (multi-turn/tools/media reject ParseInput/64); system → --system-prompt (always passed, "" for none); reasoning → --effort; extra dropped (documented inverse of the forward valve); wire always streams, stream tri-state picks the fold (decode_full = line replay).
- Decode: stream_event payloads ARE Anthropic Messages SSE events → delegated to the anthropic decoder (one parser). result = terminator + error envelope; kind from api_error_status via from_http_status, else the assistant line's authentication_failed tag (DecodeState.error_tag) → Auth/77, else Transport/69.
- Discovery: Protocol::models_shape → Option<ModelsShape>; claude_code declines --list-models with Config/78 + next-move message; learn-on-success fills the cache (verified live).

## Real pass-through proof (this machine, 2026-07-21)
$ bz --provider claude-code -m sonnet "say pong"
pong                                   (exit 0)
$ bz --provider claude-code "say pong"      # bare, after learn-on-success (last_used=sonnet)
pong                                   (exit 0)
$ bz --provider claude-code --json "reply with exactly: pong"
{"type":"message_start","v":1,"id":"msg_011CdGQYUn88GpartPzYH21m","model":"claude-sonnet-5","role":"assistant"} ... {"type":"finish","reason":"stop"} (exit 0)
$ bz --list-models --provider claude-code
provider `claude-code` has no models listing; pass --model verbatim — a model that succeeds is learned into the cache (exit 78)

Golden fixtures are the real captured streams (tests/fixtures/claude_code_basic.ndjson, claude_code_error_loggedout.ndjson). No follow-up balls needed: the coherent core (data plane + errors + decline) is whole.