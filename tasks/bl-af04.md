+++
title = "curate the 0.0.19 changelog entry: one line per user-visible change, dump merged into the hand-authored lines"
created = 1792729350
updated = 1792729352
claimant = "Gelded"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["docs"]
+++
release.md §6 step 2: release-plz prepended its commit-subject dump above the [Unreleased] prose that rode into 0.0.19, leaving two '### Changes' blocks (the dump and the curated lines). Fold them: keep one line per user-visible change in Keep-a-Changelog form naming its ball id; drop gate/infra/ops-record commits (bl-686d, bl-a5e8, bl-fb8c, bl-0641). Docs-only.