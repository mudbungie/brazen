+++
title = "release-plz.yml header comment says 'Three jobs' but there are four: describe prune-release-branches"
created = 1784699271
updated = 1784699271
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
The header comment in .github/workflows/release-plz.yml opens with 'Three jobs:' and bullets release-pr, release/publish and release-binaries. The prune-release-branches job was appended later and is not counted or described. Fix: say 'Four jobs:' and add a matching fourth bullet. Comment-only change.