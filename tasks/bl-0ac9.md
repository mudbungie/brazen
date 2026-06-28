+++
title = "Trim speculative --exit-on-retryable/EX_TEMPFAIL parenthetical from architecture.md §8 (no-speculative-API)"
created = 1782666867
updated = 1782666871
claimant = "plane"
priority = 2
tags = ["design"]
+++
Follow-up to bl-3ce8. §8's exit-code decision names a concrete unbuilt mechanism (`--exit-on-retryable` -> `EX_TEMPFAIL` 75) as the hypothetical future escape hatch. Per AGENTS.md 'new flags are a smell / don't spec speculative API', trim the parenthetical to leave only the load-bearing claim: the escape hatch is an opt-in FLAG, not a new exit code. Keep 'deferred, severable, built only on demand, weighed against just use --json'.