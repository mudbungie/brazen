//! The OS-specific corner (arch §7.3, §10): the one place a target's identity is
//! consulted. Kept tiny and behind pure functions so the only conditional in the
//! whole codebase — the browser-launch argv — is tested *as data* on one runner,
//! never executed. The real `Command::spawn`/`SIGPIPE` syscalls live in the `bz`
//! shim (the sole uncovered file); this module is pure and 100%-covered.

pub mod browser;

pub use browser::browser_argv;
