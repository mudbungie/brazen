+++
title = "Design: OAuth/subscription auth injects its required system preamble automatically"
created = 1781679365
updated = 1781680574
claimant = "Aunt"
parent = "bl-ce84"
priority = 2
tags = ["ergonomics"]
+++
A Claude-Code-scoped OAuth token is rejected unless the request's system prompt leads with 'You are Claude Code, Anthropic's official CLI for Claude.' Today the user must type --system that. It's a property of the auth mode, so the capability should supply it — but that's a BODY edit, and auth.apply works on the already-encoded WireRequest (header-only). Needs a clean seam: e.g. the resolved OAuth credential contributes a required-preamble that ENCODE prepends, so auth identity and request-shaping stay decoupled. Design first, then implement. Removes the magic --system from the one-liner.