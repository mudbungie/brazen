+++
title = "ollama_chat protocol rejects tool_result content — 'user accepts only text content'"
created = 1784955707
updated = 1785124044
claimant = "Honorifics"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Filed from lernie release evaluation (lernie bl-a227 context). A canonical request whose trailing user message carries a tool_result block is refused by the local (ollama_chat) protocol with ParseInput 'user accepts only text content', so any lernie agent using tools cannot complete step 2 over the local provider row. Ollama's /api/chat supports tool role messages (and tool_calls on assistant) — map canonical tool_result blocks to the wire's tool messages instead of rejecting. Reproduced with bz 0.0.3; check 0.0.4 behavior first.