+++
title = "Design: Google sign-in row (Code Assist backend) — OAuth2 row + envelope protocol + project onboarding"
created = 1790143957
updated = 1790392636
claimant = "Incomes"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["design"]
+++
DESIGN DELIVERED 2026-09-25 (docs only): auth.md §7.1 client_secret field + §7.5 one-builder note + §8 test row + NEW §11 recipe (Antigravity client, license gate = user-agent beta header, live verification table); providers.md NEW §4.10 google_cloudcode dialect + §9 CR-CC + §11 summary; architecture.md §11 module line; README dialect line. Probe verdict: bare {model, request} envelope works for text AND image (1.5MB JPEG) on daily-cloudcode-pa; UA must start with 'antigravity'; project/requestType/requestId all optional; non-stream works; GET models 404. Implementation lanes: bl-<secret> client_secret pair on every Grant; bl-<proto> google_cloudcode module + ProtocolId arm + registry; then recipe row in operator config + live bz --login + image e2e (Incomes).