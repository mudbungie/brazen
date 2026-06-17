+++
title = "Live Ollama integration smoke test for bz (#[ignore]d, opt-in)"
created = 1781659675
updated = 1781660473
claimant = "Litigant"
priority = 30
tags = ["test"]
+++
All ollama_chat tests are offline (encode/fixture/decode-error — each header says "No network"). The only live test in the repo is bz/tests/oauth_smoke.rs (#[ignore]d, opt-in). Add the analogous live path test for Ollama, the one provider that runs locally with no API key cost.

Scope:
- New #[ignore]d test (e.g. bz/tests/ollama_smoke.rs) that skips unless a gate env var is set (mirror oauth_smoke's BZ_SMOKE_* pattern, e.g. OLLAMA_SMOKE=1) and/or the server answers GET http://localhost:11434/api/version.
- Drive the real binary (CARGO_BIN_EXE_bz) end-to-end: pipe a canonical request with --provider ollama --model <small model> through bz, assert a non-empty streamed completion decodes (terminal MessageStop / non-empty text).
- Document the run line in the test header: cargo test -p bz --test ollama_smoke -- --ignored --nocapture, and the prereqs (ollama serve + model pulled).

Context (this session): local Ollama is installed as a persistent systemd service on this laptop; the live bz->ollama wire path is already verified by hand (model-not-found round-trips to exit 69). Models pulling: llama3.2:3b (fast) + qwen2.5:7b-instruct. NOTE: the ollama provider row is auth=bearer, so bz currently needs --api-key dummy even though Ollama ignores it (separate untracked gap vs defaults.toml's 'tolerated missing key' comment) — the test must pass a dummy key until that is fixed.