+++
title = "Add BRAZEN_TOP_P env mapping (+ config.md row + config_env.rs test rows)"
created = 1782204035
updated = 1782204035
parent = "bl-3d74"
tags = ["bug", "lane2"]
+++
src/config/env.rs:48-54 maps thinking/max_tokens/temperature/stream/timeouts but has NO top_p line, so the env layer silently cannot set top_p though it is first-class on flag (cli.rs:119), file (partial_de.rs:110), every encoder, and --dump-config. Setting BRAZEN_TEMPERATURE and BRAZEN_TOP_P drops top_p. The missing test row is WHY coverage never caught it. FIX: add top_p via parse_scalar(BRAZEN_TOP_P, env) to partial_from_env; add the BRAZEN_TOP_P|top_p row to specs/config.md (~189-200); add a BRAZEN_TOP_P value row AND a top_p entry to the unparseable-scalar/BadValue table in tests/config_env.rs (~19,81).