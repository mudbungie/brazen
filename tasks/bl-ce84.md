+++
title = 'Ergonomics: bz "question" should just work — no hand-rolled provider config to reach a model'
created = 1781679360
updated = 1781679363
priority = 1
tags = ["ergonomics"]

[[blockers]]
id = "bl-2485"
on = "close"

[[blockers]]
id = "bl-8058"
on = "close"
+++
North star (README): 'pipe in a question; it speaks the answer.' Today, reaching Anthropic with the credential actually on the box (a Claude Code OAuth token) needs a hand-written provider override + extracted token + a magic system preamble + explicit --provider. With a normal sk-ant-api key it's already near-zero-config, so the roughness is concentrated in the OAuth path + discovery + routing. Target: `bz -m claude-haiku-4-5-20251001 "question"` after a one-time `bz login`. NOT real burdens (don't chase): max_tokens (shipped row default), messages (the positional prompt). Children are the real gaps.