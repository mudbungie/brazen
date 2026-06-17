+++
title = "make smoke: cover all paths (currently only happy-path --stream/api-key/positional/text)"
created = 1781678183
updated = 1781678203
priority = 2
tags = ["smoke"]

[[blockers]]
id = "bl-61a6"
on = "close"

[[blockers]]
id = "bl-0ab8"
on = "close"

[[blockers]]
id = "bl-e99e"
on = "close"
+++
scripts/smoke.sh probes 6 provider rows with ONE shape each: positional prompt, `--stream`, env API-key, default text sink, asserting exit 0 + non-empty. That leaves whole classes of the data plane unexercised live — the [bl-ad92]/[bl-20d5] stream bug slipped precisely because no live path caught it. Children enumerate the missing smoke paths. (Distinct from bl-04dc's lib-level conformance suite — this is the shell smoke harness.) Auth coverage today: ApiKey (anthropic/google), Bearer (openai/mistral/responses), None (ollama). Missing: OAuth2/SSO, output modes, error path, stdin input.