+++
title = "Hand-author CHANGELOG.md for 0.0.1 + correct the 'conventional-commit history' claim"
created = 1782204018
updated = 1782204018
parent = "bl-3d74"
tags = ["docs", "lane1"]
+++
release-plz.yml:7 and README.md:371 both say the release PR 'writes the CHANGELOG from the conventional-commit history', but only 3/137 commits are conventional and there is no CHANGELOG.md/cliff.toml, so git-cliff emits unstructured/empty notes and won't drive bumps. FIX: hand-author a deliberate CHANGELOG.md for 0.0.1 (release-plz prepends to an existing file; summarize the feature set from README 'What works today'); soften the README+workflow wording to match reality. NOTE: adopting conventional commits (or a cliff.toml catch-all) is a separate decision for the user — do not change commit policy here.