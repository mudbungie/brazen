+++
title = "Document process-per-call economics + the sanctioned lib-embed improvement path (doc-only)"
created = 1783472145
updated = 1783472145
priority = 7
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["docs"]
+++
Owner ruling 2026-07-08: 'yes, document it. It's possibly worth looking at ways to allow the improvement of those mechanics later, but that's probably a different compile target using bz as a library.'

The specs assert one-round-trip-per-process as a purity win but never state the cost for a high-frequency embedder (arch-review audit finding): every subprocess call pays process spawn + a fresh ureq Agent + full TLS handshake (~50-150ms) + embedded defaults.toml re-parse + config-file read + model-cache file read; HTTP keep-alive / connection reuse is structurally unavailable to a subprocess consumer; N-concurrency = N processes by doctrine (§2/§12).

Deliverable, DOC-ONLY (no mechanism, no new promises): (1) an honest §12 tradeoff paragraph in architecture.md — the cost magnitude vs multi-second generation latency (negligible for agent turns), where it stops being negligible (high-frequency short calls), and the doctrine (the harness owns process lifecycle). (2) a README embedding note: the sanctioned improvement path is the TYPED LIBRARY surface — a Rust embedder holding HttpTransport across generate() calls gets connection reuse for free (verify this claim in src/native/transport.rs before writing it: if the ureq Agent lives on HttpTransport, reuse follows from holding the transport; if it does not, say what is true instead). Per the owner: improved call mechanics belong to 'a different compile target using bz as a library', NOT a daemon/serve mode — write that boundary down so the serve-mode door is documented shut.