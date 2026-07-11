+++
title = "Ingress wave 1: fail-open replay stash (XDG cache, one file per key)"
created = 1783745036
updated = 1783745055
claimant = "agent-stash"
priority = 12
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Implements specs/ingress.md par.5 verbatim. Deliverable: a replay-stash module (suggest src/store/replay.rs or src/ingress/stash.rs — keep disjoint from other lanes' files) behind the existing seam discipline (no std::env/now() in lib: XDG root and Clock injected). API: stash(key, payload-blocks) / recall(key) -> Option, write=temp+rename atomic, one file per key under $XDG_CACHE_HOME/brazen/replay/, best-effort prune on write of entries older than 7 days (mtime is the record), missing file = None (the fail-open path, never an error). Keys: tool-call id for tool-bearing turns, content hash for non-tool assistant turns (hashing helper lives here so decoder and encoder share one definition). Tests: hit, miss, prune, rename atomicity, concurrent writers (ingress.md par.14). 100% coverage, 300-line cap.