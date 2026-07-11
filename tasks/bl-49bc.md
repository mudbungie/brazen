+++
title = "Ingress wave 2: anthropic_messages ingress dialect (codec pair only)"
created = 1783745038
updated = 1783745038
priority = 4
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"

[[blockers]]
id = "bl-6cb4"
on = "claim"
+++
specs/ingress.md par.12 wave 2: add the anthropic_messages ingress codec pair (request decoder + event/SSE encoder incl. anthropic-native SSE event framing and error envelope). All par.3-par.10 machinery (ladder, lossy knob, stash, listener, routing) is reused untouched — this ball adds ONLY the codec pair + its goldens + real-SDK driver. Extend ingress.md with any anthropic-specific narrowings discovered (documented, never silent).