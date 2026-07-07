+++
title = "CUT-BLOCKER (0.0.x): set CHANGELOG.md [0.0.1] date to the actual publish date (currently 2026-06-23, tree has 2026-06-28 commits; CHANGELOG ships in the tarball)"
created = 1782688498
updated = 1783466206
claimant = "flint-changelog"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Cut-blocker for the EOD 0.0.x publish (the ONLY hard one the review found besides the verb/flag spec-note). CHANGELOG.md:13 reads '## [0.0.1] — 2026-06-23' but the 0.0.1 tree contains 2026-06-28 commits, and CHANGELOG.md is NOT in Cargo.toml's exclude list -> the wrong/premature date ships in the IMMUTABLE crates.io tarball. The CHANGELOG is hand-authored ABOVE release-plz's prepend point (see its header), so editing this date is safe and does not fight release-plz.

For the RELEASE OWNER (Dialectic) to set at cut time to match the actual publish date (2026-06-28 if cut today). Trivial one-line edit; flagged separately because the right value is whatever date the cut actually happens. Touch only CHANGELOG.md.