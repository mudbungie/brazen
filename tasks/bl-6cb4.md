+++
title = "Ingress wave 1: bz --serve listener + --in filter + pseudo-routes + stash wiring"
created = 1783745037
updated = 1783745767
claimant = "agent-listen"
priority = 12
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"

[[blockers]]
id = "bl-d2cc"
on = "claim"

[[blockers]]
id = "bl-3fc7"
on = "claim"

[[blockers]]
id = "bl-2829"
on = "claim"
+++
Implements specs/ingress.md par.7 (listener), par.8 (pseudo-routes), par.11 (--in), and wires the stash (par.5) into decode/encode join points. Deliverables: (1) --serve control-short-circuit flag entering a thread-per-connection accept loop over a Listener trait seam (accept -> impl Read+Write; main wires TcpListener; tests wire in-memory duplex) — hand-rolled minimal HTTP/1.1 (request line, headers, Content-Length body in; status+headers+body/SSE out; keep-alive serial per connection); bearer auth when token set (401 in dialect envelope), client api keys ignored; non-loopback-without-token already refused at config (other ball). Per request: decode_request -> generate -> encode_response; nothing inside generate knows it is served. Mid-stream client disconnect kills only that connection's upstream. SIGINT/SIGTERM ends the loop. (2) GET /v1/models from the existing model cache + union of every row's model_aliases keys; anything else = dialect 404. (3) --in DIALECT one-shot stdin filter, mutually exclusive with positional prompt and --raw=in (64). (4) Stash wiring: encoder-side stash writes, decoder-side recall + re-inject, miss -> degrade-the-knob (omit thinking that turn) exposed as thinking_replay per the lossy policy, reject override honored. Tests per ingress.md par.14 listener + stash bullets. main stays the only uncovered surface. 100% coverage, 300-line cap.