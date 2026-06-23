+++
title = "openai_responses decode comment says 'accumulates' but the fragment is emitted directly"
created = 1782206672
updated = 1782206674
claimant = "Purloining"
parent = "bl-3d74"
tags = ["docs"]
+++
Surfaced during bl-230f. src/protocol/openai_responses/decode/mod.rs:202 doc-comment says 'the fragment accumulates, NEVER parsed mid-stream', but delta() emits the fragment DIRECTLY as a ContentDelta (no accumulation). Align the code comment with the as-built emit-directly design (same reconciliation as bl-f94c/bl-230f). One-line comment, no logic change.