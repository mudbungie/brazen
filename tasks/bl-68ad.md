+++
title = "claude-code session misbehaves even with the same provider that works via the API — diagnose protocol gap vs limitation"
created = 1785731202
updated = 1785731280
claimant = "claude-code-fixer"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Operator report 2026-08-03, verbatim: 'The claude code session isn't working right, even when I provide the same provider I get out of the API. This might be a brazen bug/limitation.'

VERDICT (2026-08-03, claude-code-fixer): LIMITATION, working as specified — no brazen bug.

Root cause, with evidence from the operator's own failed sessions (yog dev workspace,
~/.local/share/yog/workspaces/dev/steps/2026080*): every failed model call ran through the
`claude-code` provider row with a tool-bearing worker role (`tools: [bash, read_file,
load_skill, message, dispatch]`), and every one died at encode with

    {"type":"error","kind":"parse_input","message":"claude_code carries no tool declarations; use the `anthropic` row for tools"}

That rejection is `src/protocol/claude_code/encode.rs:31-35`, deliberate per
specs/claude-code.md §4.1/§4.2/§9.3: the CLI's `-p` print mode cannot carry caller tool
declarations, assistant history, or media; a strip would silently change semantics, so the
dialect rejects at encode (arch §3.1). The claude-code row is single-turn, text-only,
tool-free — structurally unusable for an agentic harness (lernie sessions declare tools on
every call). One-shot text prompts work fine (verified live: `bz --provider claude-code -m
sonnet` returns pong, exit 0).

The 'same provider works via the API' half of the report: the operator flipped the worker
role to `claude-session-direct` (anthropic_messages over HTTP, borrowed claude OAuth) in
providers.yaml mid-session (yog dev workspace commit 2c7ad89), but lernie pins role config
at DISPATCH — all five failing steps of session 20260802T201749Z still show
provider=claude-code/model=fable, so claude-session-direct was never actually exercised in
a session. Verified live that the exact failing request shape (5 tools + system +
max_tokens) succeeds through `bz --json --provider claude-session-direct` (exit 0, clean
event stream). The escape works; the flip just never took effect for the running branch.
That pin-at-dispatch behavior is lernie/yog territory, not brazen — noted in the closing
report, not filed here.

Action taken: README.md providers bullet now states the narrowing (single-turn, text-only,
tool-free; rejects at encode; agentic callers need an HTTP anthropic_messages row — the
same logged-in claude credential works there via the `ambient = { format = "claude_code" }`
recipe, auth.md §317+). specs/claude-code.md already documented it (§4.1/§4.2); no code
change warranted.

Follow-up filed: expose per-row capability declines (tools/multi-turn) on the read surface
so pickers (yog model_pick) can warn at selection time instead of call time — sibling of
bl-75f7 (context window on the read surface).