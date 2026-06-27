+++
title = "Decide: should canonical Usage be #[non_exhaustive] before v=1 freezes? (additive token counters: reasoning_tokens, server_tool_use)"
created = 1782602938
updated = 1782602938
priority = 5
tags = ["interface-review"]
+++
Spun out of bl-2fd5. That task added #[non_exhaustive] to the 7 event ENUMS but (per its explicit scope) left the Usage STRUCT exhaustive. This ball decides whether Usage should also be #[non_exhaustive] before v=1 freezes the wire vocabulary.

FOR:
- The v=1 contract says the vocabulary only grows additively; a new token counter is exactly that.
- architecture.md §3.2 already DEFERS counters ('the usage.server_tool_use.* counters ... deferred until web-search') and Responses/others expose reasoning_tokens — so a future Usage field is known to be coming.
- #[non_exhaustive] makes adding a field non-breaking for downstream READERS (existing-field access keeps working).

AGAINST:
- #[non_exhaustive] forbids the literal constructor `Usage { input_tokens, .. }` from OTHER crates — incl. our own tests/ (a separate crate). Heavy churn: every test-side `Usage { .. }` literal would need `..Default::default()` or a builder. (Our in-crate src decoders are unaffected.)
- Usage is a plain all-Option data bag that consumers legitimately CONSTRUCT in their own test harnesses; they'd lose the literal ctor too.

Middle path: #[non_exhaustive] + standardize on `Usage { field: .., ..Default::default() }` everywhere (or a builder), absorbing the churn once, while we are still pre-1.0 and the cost is lowest.

Recommendation: lean FOR (do it now, pre-1.0) given the deferred counters are real — but it is a deliberate interface decision, hence this ball rather than scope-creeping bl-2fd5.