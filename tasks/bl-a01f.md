+++
title = "Export the crate version (pub const VERSION = env!(CARGO_PKG_VERSION))"
created = 1784336693
updated = 1784336693
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
lernie's load-time version guard compares bz --version against the version of the linked brazen crate. brazen exports EVENT_SCHEMA_VERSION but not its own crate version, so downstream must mirror the pin (lernie now parses its own Cargo.toml as a workaround). A pub const VERSION: &str = env!("CARGO_PKG_VERSION") in lib.rs lets the linked crate itself be the source of truth for 'what version am I linked against' — the guard then compares bz output to brazen::VERSION with no manifest parsing. One line + doc + test. Origin: lernie's brazen-seam review, 2026-07-16.