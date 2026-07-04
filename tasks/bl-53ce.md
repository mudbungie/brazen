+++
title = "Reconcile Role::System handling: anthropic-messages.md §2.3 says canonical System messages are hoisted into top-level system; the encoder DROPS them (continue; only req.system feeds body[system]). Decide the true behavior, fix code or doc"
created = 1783144896
updated = 1783144896
priority = 15
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["anthropic", "spec-drift"]
+++
Found by the bl-50e3 lane while implementing auto cache placement. specs/anthropic-messages.md §2.3 documents a hoist; src/protocol/anthropic/encode's messages loop does a bare `continue` on Role::System, so a mid-transcript System message silently vanishes from the wire. Per AGENTS.md: never assume the doc is correct — check actual work, then amend whichever is wrong. Candidate resolutions: (a) implement the hoist (append the System message's blocks to body[system], order-preserving); (b) declare mid-transcript System unsupported → ParseInput; (c) doc says drop, spec the empty-set rationale. Check the other dialects' System handling for consistency before choosing.