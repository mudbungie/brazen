+++
title = "Model->provider routing so --provider is droppable for unambiguous models"
created = 1781679367
updated = 1781679367
parent = "bl-ce84"
priority = 2
tags = ["ergonomics"]
+++
Shipped rows carry no model_aliases, so resolution can never route by model — every run needs --provider even for an unmistakable id like claude-haiku-4-5-20251001. Add routing (model_aliases entries, or a prefix/ownership scheme) so `bz -m claude-… "q"` finds anthropic with no --provider; keep the ambiguous-match→Config(78) guard. Removes --provider from the one-liner.