+++
title = "smoke: error path (bad key -> correct non-zero exit + surfaced provider error body)"
created = 1781678203
updated = 1781678922
claimant = "Unclasps"
parent = "bl-8cae"
priority = 2
tags = ["smoke"]
+++
smoke only tests the 2xx happy path. A deliberately-bad key should yield the right exit (auth 77 / provider-error mapping) AND a non-empty provider error message now that bl-5fe6 surfaces the upstream non-2xx body. Pairs with bl-5fe6 — guards the error projection live, per provider.