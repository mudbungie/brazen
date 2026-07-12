+++
title = "Ingress wave 3: native /v1/messages route so anthropic SDKs can drive bz --serve"
created = 1783833329
updated = 1783833329
priority = 6
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["ingress"]
+++
specs/ingress.md par.12 follow-through, raised by bl-49bc: the anthropic_messages ingress codec pair shipped (d6a6908) but wave-1 routing was reused untouched, so under bz --serve the codec answers only at the openai-shaped routes (POST /v1/chat/completions etc.) — a real Anthropic SDK pointed at bz --serve gets 404 on POST /v1/messages. The codec is fully exercisable today only via the --in anthropic_messages one-shot filter.

Scope: route registration + dialect selection by path — POST /v1/messages selects the anthropic_messages codec (the existing openai-shaped routes keep selecting openai_chat; path IS the dialect signal, no new config). HTTP-layer error responses on that route must wear the anthropic error envelope ({"type":"error","error":{"type","message"}}) with the correct native status mapping, not the openai envelope — note bl-49bc's documented narrowing that the anthropic envelope carries no numeric status in-band, so the HTTP status line is the only carrier (IngressState::status). Consider GET /v1/models parity (anthropic's models list shape) only if cheap; otherwise document the narrowing in ingress.md.

Machinery (ladder, lossy knob, stash, listener, filter) reused untouched — this is routing + envelope-at-the-edge only. Real-SDK acceptance driver: point the actual anthropic SDK (or a verbatim-captured SDK request) at the listener, assert a full round-trip, mirroring the wave-1/wave-2 driver precedent. Extend ingress.md par.12 (or a new par.) with the route table; amend README's --serve blurb. Goldens for the error envelope on the native route.