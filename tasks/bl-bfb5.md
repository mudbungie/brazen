+++
title = "Anthropic disable_parallel_tool_use lands as invalid top-level body key"
created = 1781648908
updated = 1781650523
claimant = "Tramming"
priority = 51
tags = ["bug"]
+++
anthropic-messages.md §2.7 says disable_parallel_tool_use 'merges onto the tool_choice object', but `extra` is a shallow TOP-LEVEL merge only (src/protocol/anthropic/encode.rs:44-46). So setting it in req.extra emits {"disable_parallel_tool_use":...} at the body top level — Anthropic does not accept it there (it belongs inside tool_choice). It is silently turned into an invalid request; a harness needing it must fall back to --raw.

Also a spec self-contradiction: §2.7 (nested merge) vs §2.2/§2.8 (extra is top-level only).

Resolve one of two ways: (a) implement a targeted fold of disable_parallel_tool_use from extra into the tool_choice object in encode, and fix §2.7's wording; or (b) drop the §2.7 nested-merge claim and document it as unsupported in v0.1 (rides --raw). Pick (a) if we want it normalized.