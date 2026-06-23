+++
title = "Fix broken 'cargo … -p bz' invocations (package is brazen, not bz)"
created = 1782204016
updated = 1782204016
parent = "bl-3d74"
tags = ["docs", "bug", "lane1"]
+++
The package selector flag picks a PACKAGE; since the crate collapse (97dbcb2) the package is 'brazen' and 'bz' is only a [[bin]], so 'cargo … -p bz' errors: 'package ID specification bz did not match any packages' (verified by cargo build -p bz). Sites: README.md:248,296,332; scripts/smoke.sh:35; tests/{live_conformance.rs:18,live_fuzz_openai.rs:17,live_oauth_openai.rs:25,live_encode_openai.rs:27,oauth_smoke.rs:21,ollama_smoke.rs:25}. FIX: rewrite '-p bz' to '-p brazen' for the test harnesses; smoke.sh:35 becomes 'cargo run -q --bin bz --'. make smoke unaffected (Makefile overrides BZ).