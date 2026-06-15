+++
title = "Input resolution + Sink projections (NDJSON/--text/--raw) + pump loop"
created = 1781559068
updated = 1781559068
priority = 63
tags = ["impl"]
+++
Implement pipeline/input.rs (open_input: stdin vs --input FILE, both Box<dyn Read>, 66 on open failure), pipeline/parse.rs (canonical-in, ParseInput/64 on malformed), and pipeline/sink.rs (NdjsonSink flushed-per-line, TextSink text-deltas-only, RawSink verbatim, the pump loop computing last-error-wins exit and mapping BrokenPipe->141). Test stdin/--input parity (Cursor == tempfile) and each mode from literal Event streams.