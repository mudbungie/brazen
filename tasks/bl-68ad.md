+++
title = "claude-code session misbehaves even with the same provider that works via the API — diagnose protocol gap vs limitation"
created = 1785731202
updated = 1785731208
claimant = "claude-code-fixer"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Operator report 2026-08-03, verbatim: 'The claude code session isn't working right, even when I provide the same provider I get out of the API. This might be a brazen bug/limitation.'

The symptom as reported: driving a claude-code session (src/protocol/claude_code/{encode,decode,mod}.rs and whatever transport/config routes to it) does not behave correctly, even when the provider supplied is the same one that works when hit through the plain API path. Same provider, two paths, different outcomes — so the suspect is the claude-code protocol handling (encode/decode, session framing, config/provider resolution for that protocol), not the provider itself.

This is an INVESTIGATION ball first: reproduce or trace the divergence between the claude-code path and the API path under one provider, identify whether it is (a) a brazen bug — fix it here, or (b) a genuine limitation of the claude-code protocol surface — document it precisely (what cannot work and why) and record the finding in this ball plus the repo docs, filing a follow-up ball if a design change is needed. Look for evidence in recent usage (lernie/yog drive brazen; check any recent session transcripts or logs if reachable). Verify all paths against the tree; this filing is from a quick grep.