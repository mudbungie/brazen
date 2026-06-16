+++
title = "Add --system flag: surface the config system prompt on the CLI"
created = 1781650757
updated = 1781650757
tags = ["impl"]
+++
Anything in a config file should be specifiable as a flag (config §2: flags/env/file are one schema in three encodings). bl-5125 wired `system` into PartialConfig/ResolvedConfig/fill_absent but only the config-file source sets it. Add a value-taking `--system` flag in src/cli.rs that sets cfg.system = Some(vec![Content::Text(value)]) — the ergonomic single-string form, mirroring the bare-string Content the file path accepts. Test in tests/cli_args.rs. Keep 100% coverage.