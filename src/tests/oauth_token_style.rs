//! The token endpoint's two wires and its refusals (auth §7.5): a row's
//! `token_style = "json"` sends every grant as one JSON object (with the sign-in's
//! `state` beside an authorization code) while the default row's bytes stay as they
//! were; and a refusal is printed in the vendor's own words — `error` as a string or
//! as an object, an unreadable body quoted with its HTTP status — without a body
//! that carries a token ever being shown.

use crate::testing::{
    Chunk, FakeBrowserLauncher, FakeCodeReceiver, FakePacer, MemoryCredStore, MockTransport,
};
use crate::tests::login_support::{run_store_io, Case, PASTE};
use crate::tests::oauth_pure_support::cfg;
use crate::{
    build_token_exchange_request, parse_token_response, AuthError, CredStore, Grant, Secret,
    TokenStyle,
};

/// The code-display row of `login_support::PASTE`, with a JSON token endpoint.
const JSON_ROW: &str = r#"
[[provider]]
name = "pasterow"
base_url = "https://api.example"
protocol = "openai_chat"
auth = "oauth2"
oauth = { authorize_url = "https://auth.example/authorize", token_url = "https://auth.example/token", token_style = "json", client_id = "cid", paste_redirect = "https://auth.example/code/callback" }
"#;

fn fatal(body: &[u8]) -> String {
    match parse_token_response(body, 0).unwrap_err() {
        AuthError::Fatal(message) => message,
        other => panic!("expected Fatal, got {other:?}"),
    }
}

/// One command line of the pasted-code sign-in against `store`, answered with
/// `status` + `body`: (exit, stderr, the requests sent).
fn line(
    argv: &[&str],
    config: &str,
    store: &dyn CredStore,
    status: u16,
    body: &[u8],
) -> (u8, String, Vec<crate::WireRequest>) {
    let tx = MockTransport::new(status, vec![Chunk::Data(body.to_vec())]);
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(1, "");
    let pacer = FakePacer::new();
    let case = Case {
        argv,
        config,
        tx: &tx,
        browser: &browser,
        receiver: &receiver,
        pacer: &pacer,
        now: 100,
        verifier: "the-verifier",
        state: "STATE123",
    };
    let (code, _stdout, stderr) = run_store_io(&case, store);
    (code, stderr, tx.requests())
}

/// Start a sign-in for `pasterow` under `config`, then paste `abc#STATE123`.
fn sign_in(config: &str, status: u16, body: &[u8]) -> (u8, String, Vec<crate::WireRequest>) {
    let store = MemoryCredStore::new();
    let (code, ..) = line(
        &["--login", "--provider", "pasterow"],
        config,
        &store,
        200,
        b"",
    );
    assert_eq!(code, 0);
    let argv = [
        "--login",
        "--provider",
        "pasterow",
        "--code",
        "abc#STATE123",
    ];
    line(&argv, config, &store, status, body)
}

const TOKEN: &[u8] = br#"{"access_token":"at-example","refresh_token":"rt","expires_in":3600}"#;

#[test]
fn a_json_row_sends_the_exchange_as_one_json_object_with_the_state() {
    let (code, _, sent) = sign_in(JSON_ROW, 200, TOKEN);
    assert_eq!(code, 0);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url, "https://auth.example/token");
    assert_eq!(sent[0].header("content-type"), Some("application/json"));
    assert_eq!(
        String::from_utf8_lossy(&sent[0].body),
        r#"{"grant_type":"authorization_code","code":"abc","redirect_uri":"https://auth.example/code/callback","code_verifier":"the-verifier","client_id":"cid","state":"STATE123"}"#
    );
}

#[test]
fn the_default_row_sends_the_same_sign_in_form_encoded_and_without_the_state() {
    let (code, _, sent) = sign_in(PASTE, 200, TOKEN);
    assert_eq!(code, 0);
    assert_eq!(
        sent[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        String::from_utf8_lossy(&sent[0].body),
        "grant_type=authorization_code&code=abc\
         &redirect_uri=https%3A%2F%2Fauth.example%2Fcode%2Fcallback\
         &code_verifier=the-verifier&client_id=cid"
    );
}

#[test]
fn a_json_row_sends_every_grant_as_json_and_escapes_what_it_carries() {
    let mut c = cfg();
    c.token_style = TokenStyle::Json;
    c.client_secret = Some("example-secret".into());
    let rt = Secret::new("rt\"1");
    let grants = [
        (
            Grant::Refresh { refresh_token: &rt },
            r#"{"grant_type":"refresh_token","refresh_token":"rt\"1","client_id":"cid","client_secret":"example-secret"}"#,
        ),
        (
            Grant::Device { device_code: "dev" },
            r#"{"grant_type":"urn:ietf:params:oauth:grant-type:device_code","device_code":"dev","client_id":"cid","client_secret":"example-secret"}"#,
        ),
        (
            // A code no redirect carried (the Codex device variant) has no state to send.
            Grant::AuthCode {
                code: "c",
                verifier: "v",
                redirect_uri: "https://auth.example/cb",
                state: None,
            },
            r#"{"grant_type":"authorization_code","code":"c","redirect_uri":"https://auth.example/cb","code_verifier":"v","client_id":"cid","client_secret":"example-secret"}"#,
        ),
    ];
    for (grant, body) in grants {
        let wire = build_token_exchange_request(&c, grant);
        assert_eq!(wire.header("content-type"), Some("application/json"));
        assert_eq!(String::from_utf8_lossy(&wire.body), body);
    }
}

#[test]
fn an_object_shaped_error_surfaces_its_type_and_message() {
    let body = br#"{"type":"error","error":{"type":"invalid_grant","message":"Invalid 'code' in request."}}"#;
    assert_eq!(fatal(body), "invalid_grant: Invalid 'code' in request.");
    assert_eq!(
        fatal(br#"{"error":{"type":"rate_limit_error"}}"#),
        "rate_limit_error"
    );
    assert_eq!(fatal(br#"{"error":{"message":"try later"}}"#), "try later");
    // An object naming neither is quoted whole rather than reported as nothing.
    assert_eq!(
        fatal(br#"{"error":{"code":7}}"#),
        r#"unreadable token response: {"error":{"code":7}}"#
    );
    // RFC 6749's own sentence rides beside its string code.
    assert_eq!(
        fatal(br#"{"error":"invalid_grant","error_description":"code expired"}"#),
        "invalid_grant: code expired"
    );
}

#[test]
fn poll_signals_are_read_in_either_shape() {
    let pending = parse_token_response(br#"{"error":{"type":"authorization_pending"}}"#, 0);
    assert_eq!(pending.unwrap_err(), AuthError::Pending);
    let slow = br#"{"error":{"type":"slow_down","message":"wait"}}"#;
    assert_eq!(
        parse_token_response(slow, 0).unwrap_err(),
        AuthError::SlowDown
    );
    let described = br#"{"error":"slow_down","error_description":"wait"}"#;
    assert_eq!(
        parse_token_response(described, 0).unwrap_err(),
        AuthError::SlowDown
    );
}

#[test]
fn a_long_refusal_is_cut_and_a_body_carrying_a_token_is_never_shown() {
    let long = "é".repeat(400);
    let message = fatal(long.as_bytes());
    assert_eq!(
        message,
        format!("unreadable token response: {}…", "é".repeat(300))
    );
    let described = format!(r#"{{"error":"x","error_description":"{long}"}}"#);
    assert_eq!(fatal(described.as_bytes()).chars().count(), 301);
    // Unreadable (`expires_in` is not a number) AND token-bearing: withheld, each key.
    for key in ["access_token", "refresh_token", "id_token"] {
        let body = format!(r#"{{"{key}":"notreal-token-value","expires_in":"soon"}}"#);
        let message = fatal(body.as_bytes());
        assert!(message.contains("it carries a token"), "{message}");
        assert!(!message.contains("notreal-token-value"));
    }
}

#[test]
fn a_refused_exchange_prints_the_status_and_the_vendors_own_words() {
    let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid request format"}}"#;
    let (code, stderr, _) = sign_in(JSON_ROW, 400, body);
    assert_eq!(code, 77);
    assert!(
        stderr.contains("login failed: HTTP 400: invalid_request_error: Invalid request format"),
        "{stderr}"
    );
    let (code, stderr, _) = sign_in(PASTE, 502, b"<html>Bad Gateway</html>");
    assert_eq!(code, 77);
    assert!(
        stderr.contains(
            "login failed: HTTP 502: unreadable token response: <html>Bad Gateway</html>"
        ),
        "{stderr}"
    );
}

#[test]
fn a_success_body_is_never_echoed() {
    let (code, stderr, _) = sign_in(JSON_ROW, 200, TOKEN);
    assert_eq!(code, 0);
    assert!(!stderr.contains("at-example"));
    // Token-bearing but unreadable: refused, and still not shown.
    let body = br#"{"access_token":"example-token-value","expires_in":"soon"}"#;
    let (code, stderr, _) = sign_in(JSON_ROW, 200, body);
    assert_eq!(code, 77);
    assert!(stderr.contains("HTTP 200: unreadable token response (it carries a token"));
    assert!(!stderr.contains("example-token-value"));
}
