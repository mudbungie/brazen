+++
title = "Ingress wave 1: canonical events -> openai_chat response encoder (SSE stream + aggregate fold)"
created = 1783745037
updated = 1783745356
claimant = "agent-encode"
priority = 12
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"

[[blockers]]
id = "bl-54c9"
on = "claim"
+++
Implements specs/ingress.md par.2 (encode_response), par.4 (runtime exposure), par.9 (error masquerade), par.10 (shape). Deliverable: src/ingress/openai_chat/encode.rs (+ split files as needed): canonical Event stream -> chat.completion.chunk SSE frames (index-carrying tool-call deltas, id on first chunk from injected Clock-derived fabricated identity, finish_reason vocabulary mapped from Finish, [DONE] sentinel, usage on final chunk iff stream_options.include_usage) AND the aggregate fold (stream:false client: the aggregate IS the stream accumulated — no second code path). Error masquerade: carried Frame.status when present else the shared ErrorKind->status table read in reverse; openai error envelope; mid-stream = error chunk then end. Lossy-adaptation exposure: top-level brazen:{adaptations:[...]} field on aggregates, SSE comment line on streams (ingress.md par.4). Stash WRITE join point exposed (payload blocks + their keys) for the listener to wire. Tests: canonical event scripts -> byte goldens; real-SDK-shape assertions per ingress.md par.14. Gated on the decoder ball because it owns the src/ingress/ skeleton (mod.rs collision avoidance). 100% coverage, 300-line cap.