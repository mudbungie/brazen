+++
title = "Spec 0001 revision: resolve the 5 open questions"
created = 1781560066
updated = 1781560066
priority = 85
tags = ["spec", "design"]
+++
Owner resolved all open questions:
1. max_tokens: per-provider-row default_max_tokens (Anthropic=4096) as DATA in defaults.toml, folded at lowest precedence (flag > config > row default); omitted when None and the API doesn't require it. No error-78 path.
2. --dump-config: redact secrets to an inert <redacted> sentinel (no ${VAR} expansion mechanism).
3. api-key-only built-in rows; OAuth left operator-configured.
4. Windows secret-at-rest: documented 0600/ACL limitation.
5. bz login: a subcommand of bz.
Promote §13 from open questions to resolved decisions; bump status to accepted.