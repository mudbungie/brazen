+++
title = "release 0.0.19 unblocked: bump rustls/webpki for 7 RUSTSEC advisories, repair the release-plz changelog step, record bl-6feb in the changelog"
created = 1792729126
updated = 1792729130
claimant = "Gelded"
priority = 1
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["release"]
+++
## Why
CI on main (e471e18, bl-6feb) fails only in `cargo audit --deny warnings`: rustls 0.23.40 (RUSTSEC-2026-0348/0349/0346/0285 → ≥0.23.46), rustls-webpki 0.103.13 (RUSTSEC-2026-0382 → ≥0.103.16), webpki-root-certs 1.0.6 + webpki-roots 1.0.7 (RUSTSEC-2026-0383/0381 → ≥1.0.10). Lockfile-only bumps. Separately the Release-plz workflow run on the same push fails in release-plz itself: 'error while building changelog: Template parse error --> 1:1 | # Changelog | = stack limit reached' — CHANGELOG.md did not change since the last green run (2026-09-29), so it is the action's release-plz version. Release PR #17 (v0.0.19) auto-merges only on green CI, and yog/litany pin brazen =0.0.18 from crates.io, so bl-6feb cannot reach yog until this lands.

## Done when
CI green on main; the release-plz workflow runs green and refreshes PR #17; CHANGELOG.md [Unreleased] carries the bl-6feb line(s).