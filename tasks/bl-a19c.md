+++
title = "ApiKey/Bearer Auth + inline-key bypass"
created = 1781559066
updated = 1781559066
priority = 64
tags = ["impl"]
+++
Implement auth/mod.rs ApiKey and Bearer apply: secret resolution order (inline_key -> store.get -> MissingCreds/77), header set from ctx.api_header (HeaderSpec data), identity refresh. Verify the inline-key path constructs no CredStore. Register ApiKey/Bearer in Registry::builtin. Table-test header bytes for x-api-key and Authorization:Bearer shapes against WireRequest goldens. Covers the entire v0.1 data-plane auth (OAuth deferred).