+++
title = "Bound the auto-push attempt in .githooks/reference-transaction (timeout ~10s)"
created = 1784784783
updated = 1784784783
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
User ruling 2026-07-22: no degraded-mode override for auto-push — the system is already convergent (push is non-fatal, idempotent, and total: the next successful push carries any backlog; git status derives the lag). The one defect is the unbounded attempt: offline, git push stalls bl close until the transport times out. Fix: cap the push in the hook, e.g. timeout 10 git push origin main, keeping the existing non-fatal WARNING path when it fails or expires. No env var, no config, no bl involvement — the bound makes the degraded path the same path, just capped. Also update the AGENTS.md close-gate #2 sentence if it describes the push as blocking/unbounded, and mirror the note about the manual escape (disable the local shim) only if it is not already documented. Hook-only change; no .rs files touched, but the close gate runs make check regardless.