//! End-to-end `run` (arch §9.6) — auth/config errors (77/78/64) and the config-file
//! layer. The `--dump-config` control path and the SIGPIPE (`BrokenPipe`→141) mapping
//! live in `run_control`. Driven by `MockTransport`; zero network.

mod run_support;

use brazen::testing::MemoryCredStore;
use brazen::{Cred, Secret, Timeouts};
use run_support::*;

// ============================ auth / config errors ============================

#[test]
fn missing_credential_is_auth_77() {
    // No model → the probe runs first and its `auth.apply` (the shared seam) fails
    // MissingCreds → 77 before any generation request.
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
fn missing_credential_on_the_generation_request_is_auth_77() {
    // A prefix-owned `--model` skips the probe, so the auth failure surfaces from the
    // GENERATION `auth.apply` (serve), the no-probe sibling of the case above.
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
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
fn credential_from_store_is_used() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-store"),
        },
    );
    let tx = ok_basic();
    let o = go(
        &["hi", "--provider", "anthropic", "--model", "claude-x"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn streaming_is_the_default_on_the_wire() {
    // No --stream and no config stream: serve requests streaming implicitly so the
    // SSE decoder in `drive` has a stream to frame (bl-20d5). The bare `bz <prompt>`
    // path now works against a framed provider without an explicit flag.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("k"),
        },
    );
    let tx = ok_basic();
    // `--model claude-x` is prefix-owned, so no model-list probe fires (this asserts
    // the implicit-stream default, not model discovery): one round-trip.
    let o = go(
        &["hi", "--provider", "anthropic", "--model", "claude-x"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(body.contains(r#""stream":true"#), "default body: {body}");
}

#[test]
fn an_explicit_stream_false_request_is_overridden_to_true() {
    // brazen ALWAYS wire-streams the canonical path (architecture §3.2): `drive`
    // can only frame a 2xx as a stream, so serve FORCES `stream:true`, overriding an
    // explicit `stream:false` (which would yield a single-JSON body the framers can't
    // cut). Exact non-stream wire control is `--raw`'s territory.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("k"),
        },
    );
    let tx = ok_basic();
    // A prefix-owned model so the request is one round-trip (no probe) — this asserts
    // the forced-stream override, not model discovery.
    let req = br#"{"model":"claude-m","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let o = go(&["--provider", "anthropic"], &[], req, &tx, &store);
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(
        body.contains(r#""stream":true"#),
        "forced-stream body: {body}"
    );
}

#[test]
fn run_stamps_the_resolved_timeouts_on_the_wire() {
    // `serve` stamps the resolved transport bounds onto the request the transport
    // consumes — the embedded `defaults.toml` floor unless a flag overrides it.
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk"),
        },
    );
    let tx = ok_basic();
    // A prefix-owned `--model` so no probe fires — this asserts the stamped timeouts on
    // the one generation request.
    let o = go(
        &["hi", "--provider", "anthropic", "--model", "claude-x"],
        &[],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(
        tx.requests()[0].timeouts,
        Timeouts {
            connect: Some(30),
            response: Some(120),
            idle: Some(300),
        }
    );

    // A flag overrides the floor; the override reaches the wire.
    let tx2 = ok_basic();
    let o2 = go(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--timeout-idle",
            "7",
        ],
        &[],
        b"",
        &tx2,
        &store,
    );
    assert_eq!(o2.code, 0);
    assert_eq!(tx2.requests()[0].timeouts.idle, Some(7));
}

#[test]
fn no_provider_resolved_is_config_78() {
    let o = go(&["hi", "--json"], &[], b"", &ok_basic(), &empty_store());
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains("no provider resolved"));
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
        &["hi", "--json", "--provider", "oauthy"],
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
