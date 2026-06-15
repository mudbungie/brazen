+++
title = "SSE / NDJSON decoder & DecodeState spec"
created = 1781559056
updated = 1781567934
priority = 76
tags = ["spec", "design"]
+++
The shared transport-framing layer behind Protocol::framing(): the SseDecoder (blank-line frame split, event:/data: extraction, partial-frame and partial-UTF-8 buffering, recognition of both Anthropic named message_stop and OpenAI data:[DONE]), the NDJSON line-framer (Ollama), and the Identity framer (--raw). Defines Frame, DecodeState (caller-owned open-block indices + cumulative usage), and the adversarial-rechunking determinism contract that every protocol decode must satisfy.