//! `bz login --browser` — the AuthCode + loopback flow (RFC 8252 / auth §7.4, §8):
//! `FakeBrowserLauncher` records the authorize URL (never execs), `FakeCodeReceiver`
//! serves a canned callback, `parse_callback` CSRF-checks it, and `MockTransport`
//! serves the token exchange — all offline. A CSRF mismatch and a browser-launch
//! failure are 77 with NO token exchange.

mod login_support;

use brazen::testing::{FakeBrowserLauncher, FakeCodeReceiver, FakePacer, MockTransport};
use brazen::{Cred, CredStore};
use login_support::{run, run_store, Case, FailBrowser, FailPutStore, FailReceiver, FULL};

const TOKEN: &[u8] = br#"{"access_token":"at-browser","refresh_token":"rt","expires_in":3600}"#;

fn case<'a>(
    argv: &'a [&'a str],
    tx: &'a MockTransport,
    browser: &'a dyn brazen::BrowserLauncher,
    receiver: &'a FakeCodeReceiver,
    pacer: &'a FakePacer,
    state: &'a str,
) -> Case<'a> {
    Case {
        argv,
        config: FULL,
        tx,
        browser,
        receiver,
        pacer,
        now: 100,
        verifier: "the-verifier",
        state,
    }
}

#[test]
fn browser_flow_logs_in_and_persists_the_cred() {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(8080, "code=AUTHCODE&state=STATE123");
    let pacer = FakePacer::new();
    let (code, stderr, store) = run(case(
        &["login", "claudeauth", "--browser"],
        &tx,
        &browser,
        &receiver,
        &pacer,
        "STATE123",
    ));

    assert_eq!(code, 0);
    assert!(stderr.contains("logged in to `claudeauth`"));
    // The browser was launched with a PKCE-S256 authorize URL at the loopback port.
    let opened = browser.opened();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].contains("code_challenge_method=S256"));
    assert!(opened[0].contains("127.0.0.1%3A8080%2Fcallback"));
    // The code was exchanged and the cred persisted.
    let sent = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(sent.contains("grant_type=authorization_code"));
    assert!(sent.contains("code=AUTHCODE"));
    assert!(sent.contains("code_verifier=the-verifier"));
    match store.get("claudeauth").unwrap() {
        Cred::OAuth2 {
            access_token,
            expires_at,
            ..
        } => {
            assert_eq!(access_token.expose(), "at-browser");
            assert_eq!(expires_at, 3_700); // now(100) + 3600
        }
        _ => panic!("expected OAuth2 cred"),
    }
}

#[test]
fn csrf_mismatch_is_77_with_no_token_exchange() {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FakeBrowserLauncher::new();
    // The receiver echoes a state that does NOT match the one we generated.
    let receiver = FakeCodeReceiver::new(9, "code=X&state=EVIL");
    let pacer = FakePacer::new();
    let (code, _stderr, store) = run(case(
        &["login", "claudeauth", "--browser"],
        &tx,
        &browser,
        &receiver,
        &pacer,
        "STATE123",
    ));
    assert_eq!(code, 77);
    // Never proceeded to token exchange, never persisted.
    assert!(tx.requests().is_empty());
    assert!(store.get("claudeauth").is_none());
}

#[test]
fn invalid_grant_on_exchange_is_77() {
    let tx = MockTransport::new(
        400,
        vec![brazen::testing::Chunk::Data(
            br#"{"error":"invalid_grant"}"#.to_vec(),
        )],
    );
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(9, "code=X&state=STATE123");
    let pacer = FakePacer::new();
    let (code, _stderr, store) = run(case(
        &["login", "claudeauth", "--browser"],
        &tx,
        &browser,
        &receiver,
        &pacer,
        "STATE123",
    ));
    assert_eq!(code, 77);
    assert!(store.get("claudeauth").is_none());
}

#[test]
fn pending_signal_on_exchange_is_an_unexpected_poll_signal_77() {
    // The auth-code path treats a poll signal as a fatal "unexpected" outcome.
    let tx = MockTransport::ok(vec![br#"{"error":"authorization_pending"}"#]);
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(9, "code=X&state=STATE123");
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(case(
        &["login", "claudeauth", "--browser"],
        &tx,
        &browser,
        &receiver,
        &pacer,
        "STATE123",
    ));
    assert_eq!(code, 77);
    assert!(stderr.contains("unexpected poll signal"));
}

#[test]
fn persist_failure_after_login_is_77() {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(9, "code=X&state=STATE123");
    let pacer = FakePacer::new();
    let (code, stderr) = run_store(
        &case(
            &["login", "claudeauth", "--browser"],
            &tx,
            &browser,
            &receiver,
            &pacer,
            "STATE123",
        ),
        &FailPutStore,
    );
    assert_eq!(code, 77);
    assert!(stderr.contains("could not persist credential"));
}

#[test]
fn loopback_receiver_failure_is_77() {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FakeBrowserLauncher::new();
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(Case {
        argv: &["login", "claudeauth", "--browser"],
        config: FULL,
        tx: &tx,
        browser: &browser,
        receiver: &FailReceiver,
        pacer: &pacer,
        now: 0,
        verifier: "v",
        state: "STATE123",
    });
    assert_eq!(code, 77);
    assert!(stderr.contains("loopback receiver failed"));
}

#[test]
fn browser_launch_failure_is_77() {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FailBrowser;
    let receiver = FakeCodeReceiver::new(9, "code=X&state=STATE123");
    let pacer = FakePacer::new();
    let (code, stderr, _store) = run(case(
        &["login", "claudeauth", "--browser"],
        &tx,
        &browser,
        &receiver,
        &pacer,
        "STATE123",
    ));
    assert_eq!(code, 77);
    assert!(stderr.contains("could not launch browser"));
    // Never reached the receiver / token exchange.
    assert!(tx.requests().is_empty());
}
