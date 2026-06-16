//! End-to-end `run` (arch §9.6) — auth/config errors (77/78/64), the config-file
//! layer, the `--dump-config` control path, and the SIGPIPE (`BrokenPipe`→141)
//! mapping. Driven by `MockTransport`; zero network.

mod run_support;

use brazen::testing::MemoryCredStore;
use brazen::{Cred, Secret};
use run_support::*;

// ============================ auth / config errors ============================

#[test]
fn missing_credential_is_auth_77() {
    let o = go(
        &["hi", "--json", "--provider", "anthropic"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 77);
    assert!(o.stdout.contains(r#""auth""#));
}

#[test]
fn credential_from_store_is_used() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-store"),
        },
    );
    let tx = ok_basic();
    let o = go(&["hi", "--provider", "anthropic"], &[], b"", &tx, &store);
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn no_provider_resolved_is_config_78() {
    let o = go(&["hi", "--json"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains("no provider resolved"));
}

#[test]
fn encode_rejecting_non_text_system_is_parse_input_64() {
    let stdin = br#"{"system":[{"type":"image","source":{"kind":"url","url":"http://x"}}],"messages":[{"role":"user","content":"hi"}]}"#;
    let o = go(
        &["--json", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        stdin,
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 64);
    assert!(o.stdout.contains("system accepts only text"));
}

#[test]
fn unregistered_auth_model_is_config_78() {
    let cfg = temp(
        r#"
[[provider]]
name = "oauthy"
base_url = "https://x"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
"#,
    );
    let o = go(
        &["hi", "--json", "--provider", "oauthy"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains("auth model not supported"));
}

#[test]
fn malformed_config_file_is_config_78() {
    let cfg = temp("this is not = valid = toml ===");
    let o = go(
        &["hi"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("malformed config"));
}

#[test]
fn config_file_provider_row_completes_a_run() {
    // A present, valid file is the file layer — its row routes with no flag.
    let cfg = temp(
        r#"
provider = "anthropic"
api_key = "sk-file"
"#,
    );
    let o = go(
        &["hi"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");
}

#[test]
fn bad_env_scalar_is_config_78() {
    let o = go(
        &["hi", "--provider", "anthropic", "--api-key", "sk"],
        &[("BRAZEN_OUTPUT", "bogus")],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("BRAZEN_OUTPUT"));
}

#[test]
fn unknown_flag_is_usage_64() {
    let o = go(&["--bogus"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 64);
    assert!(o.stderr.contains("unknown flag"));
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
    let code = run_broken_pipe(
        &["hi", "--json", "--provider", "anthropic", "--api-key", "sk"],
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
