+++
title = "smoke: assert output-mode contracts (--json canonical event stream, --raw passthrough)"
created = 1781678202
updated = 1781678202
parent = "bl-8cae"
priority = 2
tags = ["smoke"]
+++
smoke uses the default text sink and only checks non-empty bytes. The primary contract is the canonical event stream: a --json probe should assert a well-formed MessageStart…End sequence (the documented vocabulary), and a --raw probe should assert verbatim provider bytes pass through. Catches decode/projection regressions text-only smoke misses.