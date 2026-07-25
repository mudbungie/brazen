+++
title = "Backfill v0.0.3 GitHub Release + bz binaries"
created = 1784954391
updated = 1784954391
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
crates.io has brazen 0.0.3 and tag v0.0.3 is pushed, but gh release list stops at v0.0.2 — no GitHub Release, no prebuilt bz binaries for 0.0.3. Do: gh release create v0.0.3 with the CHANGELOG.md 0.0.3 section as notes, then gh workflow run Release-plz -f binaries_tag=v0.0.3 (the documented backfill path: 'workflow_dispatch with binaries_tag backfills an existing tag') and watch the 7 release-binaries jobs attach archives. No code diff expected.