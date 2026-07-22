+++
title = "Auto-push main to origin on commit (post-commit hook)"
created = 1784698761
updated = 1784698762
claimant = "Reverend"
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
User ruling 2026-07-21: replace the manual-push-by-design policy with automatic push. Mechanism: versioned .githooks/post-commit that pushes origin main only when the just-made commit landed on main (symbolic-ref check; skip detached HEAD and work/* worktree branches), non-fatal and loud on stderr if the push fails (offline etc. must never wedge a commit). Update AGENTS.md close-gate #2 which currently says pushing is a deliberate manual step with no auto-push hook. Note: clones wired via make hooks (core.hooksPath .githooks) get it free; clones using a local hooks-local chain need a one-line post-commit shim there (local, unversioned).