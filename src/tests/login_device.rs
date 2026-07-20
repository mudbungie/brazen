//! `bz --login` device-code flow (RFC 8628 / auth §7.3, §8) + flag-sourced provider
//! resolution. `FakeClock` + `ScriptedTransport` drive the canned
//! `authorization_pending → slow_down → success` sequence with `FakePacer`
//! recording the intervals — no sleeping. The deadline, the missing device
//! endpoint, the absent oauth row, and a bad flag are all surfaced (77 / 78 / 64).

use crate::testing::{
    FakeBrowserLauncher, FakeCodeReceiver, FakePacer, MockTransport, ScriptedTransport,
};
use crate::tests::login_support::{
    run, run_store_io, Case, DEVICE_NO_SCOPE, FULL, NO_DEVICE, NO_OAUTH,
};
use crate::{Cred, CredStore};

const DEVICE_AUTH: &[u8] =
    br#"{"device_code":"dc","user_code":"WXYZ-1234","verification_uri":"https://verify.example","expires_in":900,"interval":5}"#;

fn dev_case<'a>(
    argv: &'a [&'a str],
    config: &'a str,
    tx: &'a dyn crate::Transport,
    pacer: &'a FakePacer,
    now: u64,
) -> Case<'a> {
    // The device flow uses neither browser nor receiver; pass inert fakes.
    Case {
        argv,
        config,
        tx,
        browser: BROWSER.get_or_init(FakeBrowserLauncher::new),
        receiver: RECEIVER.get_or_init(|| FakeCodeReceiver::new(0, "")),
        pacer,
        now,
        verifier: "v",
        state: "s",
    }
}

use std::sync::OnceLock;
static BROWSER: OnceLock<FakeBrowserLauncher> = OnceLock::new();
static RECEIVER: OnceLock<FakeCodeReceiver> = OnceLock::new();

#[test]
fn device_flow_polls_pending_then_slow_down_then_succeeds() {
    let tx = ScriptedTransport::new(vec![
        (200, DEVICE_AUTH.to_vec()),
        (200, br#"{"error":"authorization_pending"}"#.to_vec()),
        (200, br#"{"error":"slow_down"}"#.to_vec()),
        (
            200,
            br#"{"access_token":"at-dev","refresh_token":"rt","expires_in":3600}"#.to_vec(),
        ),
    ]);
    let pacer = FakePacer::new();
    let (code, stderr, store) = run(dev_case(
        &["--login", "--provider", "claudeauth"],
        FULL,
        &tx,
        &pacer,
        0,
    ));

    assert_eq!(code, 0);
    // The user_code + verification_uri were printed to stderr.
    assert!(stderr.contains("WXYZ-1234"));
    assert!(stderr.contains("verify.example"));
    // slow_down raised the interval by 5 s, cumulatively: 5, 5, then 10.
    assert_eq!(pacer.waited(), vec![5, 5, 10]);
    match store.get("claudeauth").unwrap() {
        Cred::OAuth2 { access_token, .. } => assert_eq!(access_token.expose(), "at-dev"),
        _ => panic!("expected OAuth2 cred"),
    }
}

#[test]
fn device_flow_stops_at_the_deadline() {
    // expires_in = 0 → the deadline is `now`, so the first poll never fires.
    let tx = ScriptedTransport::new(vec![(
        200,
        br#"{"device_code":"dc","user_code":"U","verification_uri":"https://v","expires_in":0}"#
            .to_vec(),
    )]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(dev_case(
        &["--login", "--provider", "claudeauth"],
        FULL,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 77);
    assert!(stderr.contains("expired"));
    // Only the device-authorization request was sent; no token poll.
    assert_eq!(tx.requests().len(), 1);
    assert!(pacer.waited().is_empty());
}

#[test]
fn device_flow_fatal_poll_error_is_77() {
    // A no-scope row also covers the absent-scope arm of the device params.
    let tx = ScriptedTransport::new(vec![
        (
            200,
            br#"{"device_code":"dc","user_code":"U","verification_uri":"https://v","expires_in":900,"interval":1}"#
                .to_vec(),
        ),
        (400, br#"{"error":"invalid_grant"}"#.to_vec()),
    ]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(dev_case(
        &["--login", "--provider", "noscope"],
        DEVICE_NO_SCOPE,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 77);
    assert!(stderr.contains("device login failed"));
}

#[test]
fn malformed_device_authorization_response_is_77() {
    let tx = ScriptedTransport::new(vec![(200, b"not json".to_vec())]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(dev_case(
        &["--login", "--provider", "claudeauth"],
        FULL,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 77);
    assert!(stderr.contains("malformed device-authorization"));
}

#[test]
fn device_flow_without_device_endpoint_is_config_78() {
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(dev_case(
        &["--login", "--provider", "nodev"],
        NO_DEVICE,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 78);
    assert!(stderr.contains("--browser"));
}

#[test]
fn provider_without_oauth_block_is_config_78() {
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(dev_case(
        &["--login", "--provider", "plain"],
        NO_OAUTH,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 78);
    assert!(stderr.contains("no `oauth` config"));
}

#[test]
fn unknown_provider_is_config_78() {
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let (code, _stderr, _store) = run(dev_case(
        &["--login", "--provider", "ghost"],
        FULL,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 78);
}

#[test]
fn an_unknown_login_flag_is_usage_64() {
    // `bz --login` reuses the ONE flag parser, so an unknown flag is the same usage
    // error (64) as anywhere else — never a verb-specific argv grammar (§5.10.1).
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let (code, _stderr, _store) = run(dev_case(
        &["--login", "--provider", "claudeauth", "--bogus"],
        FULL,
        &tx,
        &pacer,
        0,
    ));
    assert_eq!(code, 64);
}

#[test]
fn login_without_a_resolvable_provider_is_config_78() {
    // `--login` requires an EXPLICITLY named provider (credential write names its
    // target): none given and none configured is `NoProvider` (→78, §5.10.1). Login
    // does NOT inherit the data plane's first-provider default — `resolve_oauth`
    // refuses an absent `provider` before `into_resolved` would apply it.
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let (code, _stderr, _store) = run(dev_case(&["--login"], FULL, &tx, &pacer, 0));
    assert_eq!(code, 78);
    assert!(tx.requests().is_empty());
}

#[test]
fn login_help_prints_the_shared_doc_to_stdout_exit_0() {
    // `bz --login --help`/`-h` is the SAME discovery short-circuit as the data plane and
    // `--list-models`: the one help doc to stdout, exit 0, BEFORE resolving a provider —
    // so it answers even with NO provider given, and it documents --browser + the flow.
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let store = crate::testing::MemoryCredStore::new();
    for flag in ["--help", "-h"] {
        let (code, stdout, stderr) =
            run_store_io(&dev_case(&["--login", flag], FULL, &tx, &pacer, 0), &store);
        assert_eq!(code, 0);
        assert!(stdout.contains("USAGE:"));
        assert!(stdout.contains("--login"));
        assert!(stdout.contains("--browser"));
        assert!(stderr.is_empty(), "help goes to stdout, not stderr");
    }
    assert!(tx.requests().is_empty(), "help does no network");
    assert!(store.get("claudeauth").is_none(), "help stores no cred");
}

#[test]
fn login_skill_prints_the_embedded_doc_to_stdout_exit_0() {
    // The third discovery probe on the login entry (§5.5): the embedded skill card to
    // stdout, exit 0, BEFORE resolving a provider — no network, no stored cred.
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let store = crate::testing::MemoryCredStore::new();
    let (code, stdout, stderr) = run_store_io(
        &dev_case(&["--login", "--skill"], FULL, &tx, &pacer, 0),
        &store,
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("agent skill card"));
    assert!(stderr.is_empty(), "skill goes to stdout, not stderr");
    assert!(tx.requests().is_empty(), "skill does no network");
    assert!(store.get("claudeauth").is_none(), "skill stores no cred");
}

#[test]
fn login_version_prints_the_package_version_to_stdout_exit_0() {
    let tx = MockTransport::ok(vec![]);
    let pacer = FakePacer::new();
    let store = crate::testing::MemoryCredStore::new();
    for flag in ["--version", "-V"] {
        let (code, stdout, stderr) =
            run_store_io(&dev_case(&["--login", flag], FULL, &tx, &pacer, 0), &store);
        assert_eq!(code, 0);
        assert_eq!(stdout, concat!("bz ", env!("CARGO_PKG_VERSION"), "\n"));
        assert!(stderr.is_empty());
    }
    assert!(tx.requests().is_empty(), "version does no network");
}
