//! `bz` — the brazen binary entry point.
//!
//! A thin shim that will wire the real (impure) implementations
//! (`HttpTransport`, the XDG `CredStore`, `SystemClock`, the browser launcher)
//! and call `brazen::run`. That spine lands in a later task; for now the entry
//! point exists so the lib/bin split is real. Excluded from coverage (see the
//! Makefile `cov` target) — all testable logic lives in the `brazen` library.
fn main() {}
