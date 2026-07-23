+++
title = "live_conformance raw-projection is invalid for contents-based dialects (google, ollama)"
created = 1784785136
updated = 1784785136
priority = 6
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["tests", "live"]
+++
The bl-34b3 live pass ran the deferred google row through tests/live_conformance.rs. Two of four applicable checks passed (json/streamed, text-projection); raw-projection FAILED with exit 69 (empty stderr).

Root cause — a HARNESS assumption, NOT a brazen bug: Row::assert_raw (tests/live_support/mod.rs:~226) feeds Row::request() — a CANONICAL, messages-shaped body ({system, messages, tools}) — through `bz --raw`. --raw bypasses encode and sends the body VERBATIM. That is valid for messages-dialects (anthropic/openai/responses/mistral all accept a messages-shaped body raw, which is why bl-8f6a saw raw-projection pass for them), but Google's generateContent wants a contents-based body ({"contents":[...]}), so the canonical body is rejected 400 -> exit 69. Ollama's row fails raw-projection too on this box, though that is entangled with a separate 'model not found' cache miss.

brazen's raw path itself is CORRECT and was proven working against live Google during bl-34b3 (a proper google-native raw body streamed native SSE verbatim; a raw fileData web-URL body produced a clean provider 400 with correct routing/auth). The gap is purely that the harness treats the canonical request as if it were provider-native for every dialect.

Fix options (pick one, principled): (a) give Row a per-dialect NATIVE raw body (contents-shaped for google, /api/chat-shaped for ollama) used only by assert_raw; or (b) gate raw-projection to dialects whose native body == the canonical messages body, and SKIP it (printed, never silent — AGENTS.md no-silent-truncation) for contents-based dialects. (a) is more faithful (actually exercises the raw wire for google); (b) is less code. Either keeps the other providers' passing raw-projection untouched.

Scope note: raw-projection is NOT one of bl-34b3's listed surfaces; all five of those are verified. This is a shared-harness follow-up, filed to avoid widening bl-34b3's blast radius into tests/live_support/.