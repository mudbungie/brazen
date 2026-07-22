//! End-to-end `run` (arch §9.6) — the `--dump-config`/`--help`/`--version` control
//! paths, the friendly bare-on-tty usage, and the SIGPIPE (`BrokenPipe`→141)
//! mapping. The auth/config/status error tables live in `run_config`/`run_failures`.
//! Driven by `MockTransport`; zero network.

use crate::tests::run_support::*;

// ============================ --help / --version ============================

#[test]
fn help_prints_one_screen_usage_to_stdout_exit_0() {
    // No provider, no config, no network — a discovery probe answers regardless.
    let o = go(&["--help"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty(), "help goes to stdout, not stderr");
    // Synopsis, the control short-circuit flags, the flag list, and the exit-code table.
    assert!(o.stdout.contains("USAGE:"));
    assert!(o.stdout.contains("--login"));
    assert!(o.stdout.contains("--list-models"));
    assert!(o.stdout.contains("--provider"));
    assert!(o.stdout.contains("--model"));
    assert!(o.stdout.contains("--json"));
    assert!(o.stdout.contains("--raw"));
    assert!(o.stdout.contains("--dump-config"));
    assert!(o.stdout.contains("EXIT CODES"));
    for code in ["0", "64", "66", "69", "70", "77", "78"] {
        assert!(o.stdout.contains(code), "exit table missing {code}");
    }
}

#[test]
fn help_wins_over_other_flags_and_version() {
    // `--help` short-circuits before resolution, so a missing provider never bites,
    // and it wins over `--version` (both → show everything).
    let o = go(
        &["--version", "--help", "--provider", "nonesuch"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("USAGE:"));
}

#[test]
fn skill_prints_the_embedded_doc_to_stdout_exit_0() {
    // The third discovery probe (§5.5): the embedded skill card to stdout, exit 0,
    // BEFORE any config/network — richer than `--help`, with worked examples. It wins
    // over `--version` (a probe answers even with a missing provider).
    let o = go(
        &["--version", "--skill", "--provider", "nonesuch"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty(), "skill goes to stdout, not stderr");
    // The card's own headings + at least one worked command that `--help` lacks.
    assert!(o.stdout.contains("agent skill card"));
    assert!(o.stdout.contains("--raw=out"));
    assert!(o.stdout.contains("Exit codes"));
}

#[test]
fn version_prints_the_package_version_exit_0() {
    let o = go(&["--version"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty());
    assert!(o.stdout.starts_with("bz "));
    assert!(o.stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn exported_version_const_is_the_crate_version() {
    // The linked crate's own version (`brazen::VERSION`), the source of truth a
    // downstream compares `bz --version` against without parsing a manifest. It IS
    // Cargo's compile-time version, is non-empty and semver-shaped (major.minor.patch),
    // and the `--version` line renders exactly it.
    assert_eq!(crate::VERSION, env!("CARGO_PKG_VERSION"));
    assert!(!crate::VERSION.is_empty());
    let parts: Vec<&str> = crate::VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "semver-shaped: {}", crate::VERSION);
    for part in parts {
        assert!(
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
            "numeric semver component: {}",
            crate::VERSION
        );
    }
    let o = go(&["--version"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.stdout, format!("bz {}\n", crate::VERSION));
}

// ============================ friendly bare invocation ============================

#[test]
fn bare_on_tty_prints_usage_to_stderr_exit_64() {
    // Interactive terminal, no prompt, no --input: nothing to read. Usage → stderr,
    // exit 64 — NOT an empty-stdin parse error.
    let o = go_tty(&[], &ok_basic(), &empty_store());
    assert_eq!(o.code, 64);
    assert!(o.stdout.is_empty(), "the hint goes to stderr, not stdout");
    assert!(o.stderr.contains("USAGE:"));
    assert!(o.stderr.contains("EXIT CODES"));
}

#[test]
fn bare_on_tty_with_a_prompt_is_not_the_usage_path() {
    // A positional prompt is a request even on a tty — the usage guard must not fire;
    // this flows into the normal pipeline (and succeeds against the happy transport).
    let o = go_tty(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(!o.stderr.contains("USAGE:"));
}

#[test]
fn piped_empty_stdin_is_still_a_parse_error_not_the_usage_hint() {
    // CRITICAL: the pipe path (tty == false) is unchanged. Empty piped stdin with no
    // prompt is still the canonical-parse error (64), never the friendly usage.
    let o = go(&[], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 64);
    assert!(!o.stdout.contains("USAGE:"));
    assert!(!o.stderr.contains("USAGE:"));
}

// ============================ --dump-config ============================

#[test]
fn dump_config_prints_merged_toml_exit_0() {
    let o = go(
        &["--dump-config", "--provider", "anthropic"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains(r#"provider = "anthropic""#));
}

#[test]
fn dump_config_with_bad_env_is_78() {
    let o = go(
        &["--dump-config"],
        &[("BRAZEN_OUTPUT", "bogus")],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("BRAZEN_OUTPUT"));
}

// ============================ SIGPIPE / BrokenPipe ============================

#[test]
fn broken_pipe_during_streaming_is_141() {
    // A prefix-owned `--model` so no probe fires and the broken pipe is hit DURING the
    // generation stream, the path this asserts.
    let code = run_broken_pipe(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &empty_store(),
    );
    assert_eq!(code, 141);
}

#[test]
fn broken_pipe_during_inband_error_is_141() {
    // Missing creds (77) writes the error to stdout under --json; the pipe breaks.
    let code = run_broken_pipe(&["--json", "--provider", "anthropic", "hi"], &empty_store());
    assert_eq!(code, 141);
}

#[test]
fn broken_pipe_during_dump_is_141() {
    let code = run_broken_pipe(
        &["--dump-config", "--provider", "anthropic"],
        &empty_store(),
    );
    assert_eq!(code, 141);
}

#[test]
fn broken_pipe_during_help_is_141() {
    // `--help`'s write-and-flush maps a closed stdout to SIGPIPE/141, like `dump`
    // (the `emit` `from_io` arm) — `bz --help | head` is never a silent 0.
    let code = run_broken_pipe(&["--help"], &empty_store());
    assert_eq!(code, 141);
}

#[test]
fn broken_pipe_during_version_is_141() {
    let code = run_broken_pipe(&["--version"], &empty_store());
    assert_eq!(code, 141);
}
