+++
title = "Restore region coverage in canonical/event.rs (hand-rolled serialize ? error-arms uncovered); decide region- vs line-gating"
created = 1782602934
updated = 1782695837
claimant = "mark"
priority = 6
tags = ["impl"]
+++
Spun out of bl-2fd5. The repo gate is `cargo llvm-cov --fail-under-lines 100` (LINE coverage). bl-2fd5 hand-rolled Serialize for ContentKind/Delta/FinishReason; the `?` operators (`serialize_map(..)?`, `serialize_entry(..)?`) create an Ok-region (covered) and an Err-early-return region (NEVER taken, because serializing to an in-memory String/Vec is infallible). So canonical/event.rs is 100% LINES but ~92% REGIONS.

Options:
1. Cover the Err arms: serialize through a deliberately-failing Serializer (or a writer that errors) so the `?` error path executes. Adds test machinery for paths that are infallible in production.
2. Refactor to remove the `?` branches where the operation can't fail (harder — serde's API is fallible by signature).
3. Decide line-gating is the contract and document WHY region 100% isn't pursued (the infallible-serialize arms are the canonical example).

Context: region coverage is ALSO <100% repo-wide (TOTAL 98.47% region at delivery: run/models.rs, run/respond.rs, encode.rs files, testing/* etc.) — this is pre-existing, not unique to event.rs. A repo-wide region gate is a much bigger effort than just event.rs.