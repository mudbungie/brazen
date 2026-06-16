#!/bin/sh
# PreToolUse(Bash) advisory docs gate.
# When a `bl close` is about to deliver a task, remind the agent to bring the
# living docs in line with the delivered change. Non-blocking: it emits
# additionalContext for the model and never denies the tool call.
# Requires `jq`. Keep it POSIX sh.
set -eu

cmd=$(jq -r '.tool_input.command // empty')

case "$cmd" in
	*"bl close"*)
		jq -n '{
		  hookSpecificOutput: {
		    hookEventName: "PreToolUse",
		    additionalContext: "Docs gate (advisory, non-blocking): before this `bl close` delivers, confirm the living docs match the change you are delivering — update specs/ (the authoritative design), README.md, and AGENTS.md as needed. The close will proceed regardless; this is only a reminder."
		  }
		}'
		;;
esac

exit 0
