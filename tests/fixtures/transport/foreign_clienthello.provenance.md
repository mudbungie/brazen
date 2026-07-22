# `foreign_clienthello.bin` — provenance

A real TLS first flight, captured verbatim from a client that is **not** brazen's stack,
so the JA3-form instrument (`tests/transport_support/tls.rs`) can be shown to measure the
ClientHello rather than return a constant (transport spec §8.3, claim 3).

| Fact | Value |
|---|---|
| Client | `curl 8.18.0 (x86_64-pc-linux-gnu) libcurl/8.18.0 OpenSSL/3.5.5 nghttp2/1.68.0` |
| Command | `curl -s -k --max-time 3 https://127.0.0.1:<port>/` |
| Captured | 2026-07-22, one `recv` off a loopback `TcpListener` that never answers |
| Bytes | 1551, the record verbatim — no edits |

It is **test data, not a target**: nothing in brazen imitates this client, and no shipped
code reads this file. Re-capture against any other stack to re-prove the same claim.
