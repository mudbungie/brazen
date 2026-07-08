+++
title = "SSE decoder robustness: non-SSE 200 body discarded as bare premature-EOF; WHATWG BOM not stripped; find_frame_end rescans O(n^2)"
created = 1783466819
updated = 1783470391
priority = 16
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["robustness"]
+++
Arch-review finding (2026-07-07), three defects in one lane (src/protocol/sse.rs + run/events/stream.rs):

1) Non-SSE 200 body: a 200 whose body is not SSE (gateway HTML, JSON error served with 200, plain text) parses to zero frames (parse_block -> None, sse.rs:90-98), terminated never sets, and the run reports generic 'premature upstream EOF' Transport/69 (stream.rs:64-76) while DISCARDING the body — including the actual upstream error text. The non-2xx path deliberately preserves the raw body verbatim in provider_detail (protocol/json.rs:118-137); the 200-with-wrong-content case deserves the same care. Fix shape (spec-argue in sse-decoder.md §9 + architecture.md §5.6): when EOF arrives with terminated=false AND zero frames were ever decoded, attach the accumulated (bounded, e.g. first N KiB) body to the premature-EOF error's provider_detail so the failure is diagnosable.

2) BOM: leading UTF-8 BOM (EF BB BF) is never stripped (zero hits in src); WHATWG SSE requires stripping it. Today it corrupts the first line's field name — for the OpenAI dialect (first block is a bare data:) the ENTIRE first frame is dropped. One-line fix at decoder init + fixture; sse-decoder.md §6.1 note.

3) find_frame_end rescans buf from index 0 on every push (sse.rs:37,57-67) — O(n^2) on a frame that never terminates, and buf is unbounded (sse.rs:33). Fix the scan to resume from a remembered offset (pure perf, no behavior change). Whether to CAP the buffer is a design question — a cap invents a new failure mode; the scan fix removes the CPU blowup; decide and document (an unbounded buffer on a hostile upstream is memory-DoS; the idle timeout never trips while bytes flow, native/transport/idle.rs:62-81).

Rechunking determinism tests must keep passing across all strategies; fixtures for each case; CHANGELOG.