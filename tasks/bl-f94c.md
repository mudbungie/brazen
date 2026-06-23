+++
title = "Remove dead decoder state: OpenBlock.buffer + DecodeState.usage (reconcile sse-decoder.md)"
created = 1782204030
updated = 1782204265
claimant = "Lane2"
parent = "bl-3d74"
tags = ["cleanup", "lane2"]
+++
OpenBlock.buffer (protocol/frame.rs:64) is written by all 5 decoders but read by NO production code (only tests); comments promise a 'fold-time parse' consumer that does not exist. DecodeState.usage (frame.rs:96) is never read/written in production (every decoder builds a fresh local Usage); specs/sse-decoder.md:156 describes a stored-snapshot mechanism that is not implemented. FIX: delete both dead fields + their write sites across the 5 decode block modules and reconcile sse-decoder.md to the as-built (emit-directly) design. Update/remove tests that read these fields. Keep make check green at 100% line cov.