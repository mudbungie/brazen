+++
title = "image output: text/pretty sinks write ./bz-<sha256[..12]>.<ext>, path on stderr; ext table read both ways"
created = 1790125224
updated = 1790125224
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Lane 4 of bl-0987 (architecture.md §5.3 'An Image block in text mode is a FILE', interactive-output.md §5 Image bullet, canonical-protocol.md §3.4). TextSink and PrettySink take a dir: &Path (run passes ".", tests a tempdir); accumulate ImageDelta per open Image block; at ContentStop decode base64 once, sha256 (sha2 already a dep), write <dir>/bz-<hex12>.<ext>, print path on stderr (PLAIN bare + newline; pretty cyan ▣ / ASCII #). Terminal flush (Error/End) DROPS a truncated accumulator, never writes. Make input.rs's extension↔media-type table one const slice read in both directions (no second table); unmapped → bin. Files: src/pipeline/sink.rs, pretty.rs, input.rs, src/run/mod.rs sink construction, tests.