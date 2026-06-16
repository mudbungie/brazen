+++
title = "Scaffold the crate: lib/bin split, canonical types, error model"
created = 1781559059
updated = 1781591767
claimant = "Feeding"
priority = 70
tags = ["impl"]
+++
Create the cargo workspace with lib `brazen` and bin `bz`. Implement canonical/request.rs, canonical/event.rs, canonical/error.rs (CanonicalError, ErrorKind, the pure retryable() and exit_code()/ExitClass tables) with full serde derives matching the spec's tag conventions. No network, no IO. Table-test the error/exit/retryable mappings exhaustively to 100%. This is the dependency root for everything else.