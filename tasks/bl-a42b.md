+++
title = "CI portability matrix + publish the crate"
created = 1781559074
updated = 1781651681
claimant = "Dickie"
priority = 58
tags = ["impl"]

[[blockers]]
id = "bl-2f13"
on = "claim"
+++
Wire the CI matrix (Linux/macOS/Windows x x86_64/aarch64 + x86_64-unknown-linux-musl), the make check gate (fmt + clippy -D warnings + llvm-cov --fail-under-lines 100), and crate metadata for publishing brazen (lib) with bz (bin). Confirm no build scripts/C deps; confirm the lib has no platform-specific code (all behind trait injection). Document the Windows secret-at-rest ACL limitation in the README.