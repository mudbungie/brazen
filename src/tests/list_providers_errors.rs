//! `bz --list-providers` error and short-circuit paths (config §6.1): an incomplete
//! row (78), a malformed file / bad env scalar (78), an unknown flag (64), the
//! `--help`/`--skill`/`--version` probes, and a failed listing write (69). Offline.

use std::io::{self, Write};

use crate::testing::MemoryCredStore;
use crate::tests::list_providers_support::{floor_argv, go, go_into};
use crate::tests::run_support::temp;

/// A row that cannot complete fails the LISTING with resolution's own message — the
/// listing shares the one `row::complete` lift, so the two cannot disagree (§6.1).
#[test]
fn an_incomplete_row_is_the_usual_78() {
    let cfg = temp("[[provider]]\nname = \"broken\"\n");
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    assert_eq!(out.code, 78);
    assert!(
        out.stderr
            .contains("provider `broken` is missing required field `base_url`"),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
}

/// A config that cannot ROUTE still LISTS (§6.1): routing checks deliberately do not
/// run, so the diagnostic verb stays usable on the config being diagnosed.
#[test]
fn a_config_that_cannot_route_still_lists() {
    let cfg = temp("temperature = \"not-a-number\"\n");
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    assert_eq!(out.code, 78, "a malformed FILE is still 78: {}", out.stderr);
    let ok = go(
        &["--list-providers", "--top-p", "0.5"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &MemoryCredStore::new(),
    );
    assert_eq!(ok.code, 0);
}

/// A bad env scalar is the usual 78 — the same projection the data plane runs.
#[test]
fn a_bad_env_scalar_is_78() {
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", "/nope.toml"), ("BRAZEN_OUTPUT", "bogus")],
        &MemoryCredStore::new(),
    );
    assert_eq!(out.code, 78);
    assert!(out.stderr.contains("BRAZEN_OUTPUT"), "{}", out.stderr);
}

/// The verb re-parses the WHOLE argv authoritatively, so an unknown flag is 64.
#[test]
fn an_unknown_flag_is_a_usage_error() {
    let out = floor_argv(&["--list-providers", "--bogus"]);
    assert_eq!(out.code, 64);
    assert!(out.stderr.contains("unknown flag"), "{}", out.stderr);
}

/// The discovery probes answer BEFORE any config read (arch §5.5) — one doc, every
/// entry: `bz --list-providers --help` self-describes exactly as `bz --help` does.
#[test]
fn the_probes_short_circuit_the_listing() {
    for (flag, needle) in [
        ("--help", "USAGE:"),
        ("--skill", "bz"),
        ("--version", "bz "),
    ] {
        let out = floor_argv(&["--list-providers", flag]);
        assert_eq!(out.code, 0);
        assert!(out.stdout.contains(needle), "{flag}: {}", out.stdout);
    }
    // The help screen documents the flag it just short-circuited for.
    assert!(floor_argv(&["--help"]).stdout.contains("--list-providers"));
}

/// A failed listing write is `Transport` (→69), the same pre-sink mapping
/// `--list-models` uses for its own listing.
#[test]
fn a_failed_write_is_69() {
    struct FailWriter;
    impl Write for FailWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("disk full"))
        }
    }
    let (code, stderr) = go_into(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &MemoryCredStore::new(),
        &mut FailWriter,
    );
    assert_eq!(code, 69);
    assert!(stderr.contains("failed to write provider list"), "{stderr}");
}
