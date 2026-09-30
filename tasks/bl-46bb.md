+++
title = "tests: one Ollama default name (llama3.2) across live tests; drop box-specific bz-smoke comment"
created = 1790733602
updated = 1790733663
claimant = "Belay"
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Cleanup after pulling llama3.2 on the operator box (ops bl-c2c0). (1) tests/ollama_smoke.rs default llama3.2:3b -> llama3.2 (one Ollama name repo-wide); drop box-specific bz-smoke comment in live_conformance. (2) live chatgpt default gpt-5.4 (retired) -> gpt-5.5 in live_conformance, live_oauth_openai, live_support/openai, live_encode_openai. (3) bl-d0bf removed the built-in google row but left it in live_conformance TABLE and smoke.sh -> both removed (+ RawBody::Contents, smoke raw_body google arm, GOOGLE_API_KEY alias, route case). (4) Pre-existing since bl-5f6e: Responses rows sent a messages-shaped --raw body (codex 400 'Unsupported parameter: system'); new RawBody::Responses (instructions+input+store:false). Verified live, no env overrides: smoke 9/0/4, live_conformance 0 failed (ollama+chatgpt), live_oauth/encode/fuzz_openai pass on gpt-5.5.