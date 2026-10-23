//! End-to-end `run` (arch §9.6) — auth/config errors (77/78/64) and the config-file
//! layer. The resolved-values-reach-the-wire tests live in `run_wire`; the
//! `--dump-config` control path and the SIGPIPE (`BrokenPipe`→141) mapping in
//! `run_control`. Driven by `MockTransport`; zero network.

use crate::tests::run_support::*;

#[test]
fn an_absent_model_against_an_empty_cache_is_config_78() {
    // The probe is dissolved (model-discovery §5): with NO `--model` and an empty cache,
    // the generation path's cache lookup hits `select_model`'s lone error — Config 78 —
    // BEFORE auth. (A resolvable model + missing creds is the auth-77 case below.)
    let o = go(
        &["--json", "--provider", "anthropic", "hi"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains(r#""config""#));
}

#[test]
fn missing_credential_on_the_generation_request_is_auth_77() {
    // A resolvable `--model` (verbatim against the empty cache) reaches `encode` and the
    // GENERATION `auth.apply` (serve), which fails MissingCreds → 77 — the single send.
    let o = go(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "hi",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 77);
    assert!(o.stdout.contains(r#""auth""#));
}

#[test]
fn no_spec_routes_to_the_first_table_row_then_hits_the_cold_cache() {
    // Zero-config `bz "hi"`: no `--provider`, no `--model`. The default is the head of
    // the TABLE (config §3.2) — the presets a credential resolves for — never a
    // catalog row that would only 77. With nothing held, the table is empty and the
    // failure is `NoProvider` (Config 78) naming the acts that fill it…
    let o = go(&["--json", "hi"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains(r#""config""#));
    assert!(
        o.stdout.contains("no provider resolved") && o.stdout.contains("bz --login"),
        "an empty table names the remedies: {}",
        o.stdout
    );
    // …and with a stored openai credential, `openai` IS the table, so it is the
    // default, and the failure moves on to the COLD-CACHE model error (still 78):
    // provider resolved, but no cached model to default to.
    let store = crate::testing::MemoryCredStore::with(
        "openai",
        crate::Cred::Bearer {
            token: crate::Secret::new("t"),
        },
    );
    let o = go(&["--json", "hi"], &[], b"", &ok_basic(), &store);
    assert_eq!(o.code, 78);
    assert!(
        o.stdout.contains("no model cache for openai"),
        "routed to the one table row, then the cold-cache model error: {}",
        o.stdout
    );
}

#[test]
fn encode_rejecting_non_text_system_is_parse_input_64() {
    let stdin = br#"{"model":"claude-x","system":[{"type":"image","source":{"kind":"url","url":"http://x"}}],"messages":[{"role":"user","content":"hi"}]}"#;
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
fn oauth2_row_without_oauth_block_is_config_78() {
    // `auth = "oauth2"` MUST be paired with an `oauth` block; resolution surfaces
    // the missing field (auth §1.3) rather than reaching `apply` mis-wired.
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
        &["--json", "--provider", "oauthy", "hi"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains("missing required field `oauth`"));
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
model = "claude-x"
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
        &["--provider", "anthropic", "--api-key", "sk", "hi"],
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
