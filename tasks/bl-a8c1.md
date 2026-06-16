+++
title = "Config schema, resolution & compiled-config spec"
created = 1781559055
updated = 1781588765
claimant = "fuzzball"
priority = 77
tags = ["spec", "design"]
+++
The PartialConfig schema (all-Option, sparse provider table), the Option::or fold (flags > env > file > embedded defaults) as data-ordered precedence, env-snapshot projection, embedded defaults.toml through the same parse path, config-file location resolution (--config > $BRAZEN_CONFIG > XDG), the missing-file-is-identity-element rule, and --dump-config (serialize merged-without-defaults, secrets elided to placeholders) as the only bridge between flag-encoding and file-encoding. Specifies into_resolved() validation errors (78) including ambiguous model->provider resolution.