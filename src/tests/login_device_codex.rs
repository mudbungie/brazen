//! `bz --login` against a row declaring `device.style = "codex"` (auth §10.8): the
//! vendor's pre-standard device-code wire, driven offline by `ScriptedTransport` +
//! `FakePacer`. Every URL the flow dials derives from the row's ONE `device.url`, the
//! poll signal is the HTTP STATUS (not an error string), success answers an
//! authorization CODE that the ordinary AuthCode exchange spends, and a refusal is
//! the provider's own body, quoted.

use crate::testing::{FakeBrowserLauncher, FakeCodeReceiver, FakePacer, ScriptedTransport};
use crate::tests::login_support::{run, Case, CODEX};
use crate::{Cred, CredStore};

const USER_CODE: &[u8] = br#"{"device_auth_id":"dai","user_code":"WXYZ-1234","interval":2}"#;
const GRANTED: &[u8] =
    br#"{"authorization_code":"ac","code_challenge":"ch","code_verifier":"ver"}"#;
const TOKEN: &[u8] =
    br#"{"access_token":"at-codex-example","refresh_token":"rt","expires_in":3600}"#;

fn go(tx: &ScriptedTransport, pacer: &FakePacer) -> (u8, String, crate::testing::MemoryCredStore) {
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(0, "");
    run(Case {
        argv: &["--login", "--provider", "codexauth"],
        config: CODEX,
        tx,
        // The codex device flow touches neither seam; inert fakes prove it.
        browser: &browser,
        receiver: &receiver,
        pacer,
        now: 0,
        verifier: "v",
        state: "s",
    })
}

fn body(wire: &crate::protocol::WireRequest) -> String {
    String::from_utf8_lossy(&wire.body).into_owned()
}

#[test]
fn codex_flow_polls_by_status_then_spends_the_code_at_the_token_endpoint() {
    let tx = ScriptedTransport::new(vec![
        (200, USER_CODE.to_vec()),
        // 403 while nobody has entered the code; 404 until the record exists. Both
        // are this wire's `authorization_pending`.
        (403, b"{}".to_vec()),
        (404, b"{}".to_vec()),
        (200, GRANTED.to_vec()),
        (200, TOKEN.to_vec()),
    ]);
    let pacer = FakePacer::new();
    let (code, stderr, store) = go(&tx, &pacer);

    assert_eq!(code, 0);
    // The human prompt names the vendor's verification PAGE, derived from the same
    // `device.url` — no second copy of the base on the row.
    assert!(
        stderr.contains("https://auth.example/codex/device"),
        "{stderr}"
    );
    assert!(stderr.contains("WXYZ-1234"), "{stderr}");
    // The row's `interval` paced every poll; nothing raised it (no `slow_down` on
    // this wire — its signal is a status, and a status never asks to slow down).
    assert_eq!(pacer.waited(), vec![2, 2, 2]);

    let reqs = tx.requests();
    assert_eq!(reqs.len(), 5);
    assert_eq!(reqs[0].url, "https://auth.example/deviceauth/usercode");
    assert_eq!(body(&reqs[0]), r#"{"client_id":"cid"}"#);
    assert_eq!(
        reqs[0].headers,
        [("content-type".to_owned(), "application/json".to_owned())]
    );
    assert_eq!(reqs[1].url, "https://auth.example/deviceauth/token");
    assert_eq!(
        body(&reqs[1]),
        r#"{"device_auth_id":"dai","user_code":"WXYZ-1234"}"#
    );
    // The tail is the ORDINARY AuthCode exchange (auth §7.5) — the same builder the
    // loopback flow ends in, against the row's `token_url`, with the vendor's
    // `deviceauth/callback` as the redirect it registered for this grant.
    assert_eq!(reqs[4].url, "https://auth.example/token");
    assert_eq!(
        body(&reqs[4]),
        "grant_type=authorization_code&code=ac&redirect_uri=https%3A%2F%2Fauth.example%2Fdeviceauth%2Fcallback&code_verifier=ver&client_id=cid"
    );
    match store.get("codexauth").unwrap() {
        Cred::OAuth2 { access_token, .. } => assert_eq!(access_token.expose(), "at-codex-example"),
        _ => panic!("expected OAuth2 cred"),
    }
}

#[test]
fn an_absent_interval_falls_back_to_the_five_second_default() {
    let tx = ScriptedTransport::new(vec![
        (200, br#"{"device_auth_id":"dai","user_code":"U"}"#.to_vec()),
        (200, GRANTED.to_vec()),
        (200, TOKEN.to_vec()),
    ]);
    let pacer = FakePacer::new();
    let (code, _stderr, _store) = go(&tx, &pacer);
    assert_eq!(code, 0);
    assert_eq!(pacer.waited(), vec![5]);
}

#[test]
fn a_refused_device_authorization_streams_the_providers_own_body() {
    // Device-code login can be switched off for a ChatGPT account or workspace in its
    // security settings; the provider refuses the FIRST call and says why. brazen
    // quotes it rather than compiling in a guess about a policy it cannot see.
    let refusal = br#"{"detail":"device code login is disabled for this workspace"}"#;
    let tx = ScriptedTransport::new(vec![(403, refusal.to_vec())]);
    let pacer = FakePacer::new();
    let (code, stderr, store) = go(&tx, &pacer);
    assert_eq!(code, 77);
    assert!(stderr.contains("HTTP 403"), "{stderr}");
    assert!(
        stderr.contains("device code login is disabled for this workspace"),
        "{stderr}"
    );
    // Refused before the prompt: no code was ever shown, and nothing was stored.
    assert!(!stderr.contains("To authorize"), "{stderr}");
    assert!(store.get("codexauth").is_none());
    assert_eq!(tx.requests().len(), 1);
    assert!(pacer.waited().is_empty());
}

#[test]
fn a_poll_status_that_is_neither_success_nor_pending_is_77() {
    let tx = ScriptedTransport::new(vec![
        (200, USER_CODE.to_vec()),
        (500, b"upstream exploded".to_vec()),
    ]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = go(&tx, &pacer);
    assert_eq!(code, 77);
    assert!(stderr.contains("device poll"), "{stderr}");
    assert!(stderr.contains("upstream exploded"), "{stderr}");
}

#[test]
fn a_malformed_usercode_response_is_77() {
    let tx = ScriptedTransport::new(vec![(200, b"not json".to_vec())]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = go(&tx, &pacer);
    assert_eq!(code, 77);
    assert!(
        stderr.contains("malformed device-authorization"),
        "{stderr}"
    );
}

#[test]
fn a_malformed_poll_success_is_77() {
    let tx = ScriptedTransport::new(vec![
        (200, USER_CODE.to_vec()),
        (200, br#"{"authorization_code":"ac"}"#.to_vec()),
    ]);
    let pacer = FakePacer::new();
    let (code, stderr, _store) = go(&tx, &pacer);
    assert_eq!(code, 77);
    assert!(
        stderr.contains("malformed device-authorization"),
        "{stderr}"
    );
}
