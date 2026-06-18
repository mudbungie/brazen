//! End-to-end `run` (arch §9.6) — the `--dump-config` control path and the SIGPIPE
//! (`BrokenPipe`→141) mapping. The auth/config/status error tables live in
//! `run_config`/`run_failures`. Driven by `MockTransport`; zero network.

mod run_support;

use run_support::*;

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
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &empty_store(),
    );
    assert_eq!(code, 141);
}

#[test]
fn broken_pipe_during_inband_error_is_141() {
    // Missing creds (77) writes the error to stdout under --json; the pipe breaks.
    let code = run_broken_pipe(&["hi", "--json", "--provider", "anthropic"], &empty_store());
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
