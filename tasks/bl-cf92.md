+++
title = "antigravity: probe the non-Gemini models the backend lists (CR-CC 5)"
created = 1790656695
updated = 1790656695
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Gap from providers.md §9 CR-CC item (5), 2026-09-28. The Cloud Code backend's model list includes non-Gemini models (claude-sonnet-4-6, claude-opus-4-6-thinking, gpt-oss-120b-medium). Per the upstream client they answer in the same Gemini wire, but brazen has never sent them a request. Probe each through the antigravity row (text, a tool call, reasoning) and record what decodes; file specific bugs if any. Low priority, curiosity-grade unless the operator wants those models.