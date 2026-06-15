+++
title = "Design spec 0001: Architecture & I/O Contract"
created = 1781558062
updated = 1781558062
priority = 90
tags = ["spec", "design"]
+++
Deliverable: specs/0001-architecture.md (living document).

Defines brazen's architecture: the canonical model (single source of truth), the Provider/Protocol/Auth adapter abstraction (severability), the I/O + streaming + POSIX contract (framing, --raw, --input, end token, exit codes, signals), config + credentials (XDG, resolution, compiled config file), auth + browser SSO/OAuth, error model, testability + 100% coverage strategy, portability matrix, and module layout (<=300-line files).

Produced design-first via a multi-agent design panel (4 architectures -> judged -> deep-dived -> synthesized). Synthesized draft committed here, then iterated like code.