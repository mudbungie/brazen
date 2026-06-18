//! The pure OAuth callback + credential-merge half (auth §7.5, §6.1, §8):
//! `parse_callback` (CSRF, missing fields, the percent codec exercised through it,
//! unknown/bare params), the loopback `query_from_request_line`, and
//! `TokenResponse::as_cred` (prior refresh/scope/account reuse when a rotation omits
//! them) — all asserted from literals with zero IO. The builders + token-response
//! parsing live in the sibling `oauth_pure`.

use brazen::{parse_callback, query_from_request_line, AuthError, Secret, TokenResponse};

#[test]
fn as_cred_reuses_prior_refresh_scope_and_account_when_omitted() {
    let rotated = TokenResponse {
        access_token: Secret::new("new-at"),
        refresh_token: Some(Secret::new("new-rt")),
        expires_at: 9_000,
        scope: Some("scope-new".into()),
        account_id: Some("acct-new".into()),
    };
    let prior = Secret::new("old-rt");
    match rotated.as_cred(&prior, &Some("scope-old".into()), &Some("acct-old".into())) {
        brazen::Cred::OAuth2 {
            refresh_token,
            scope,
            account_id,
            ..
        } => {
            assert_eq!(refresh_token.expose(), "new-rt"); // rotated replaces
            assert_eq!(scope.as_deref(), Some("scope-new"));
            assert_eq!(account_id.as_deref(), Some("acct-new")); // fresh replaces
        }
        _ => panic!("expected OAuth2 cred"),
    }
    let omitted = TokenResponse {
        access_token: Secret::new("a"),
        refresh_token: None,
        expires_at: 1,
        scope: None,
        account_id: None,
    };
    match omitted.as_cred(&prior, &Some("scope-old".into()), &Some("acct-old".into())) {
        brazen::Cred::OAuth2 {
            refresh_token,
            scope,
            account_id,
            ..
        } => {
            assert_eq!(refresh_token.expose(), "old-rt"); // prior reused
            assert_eq!(scope.as_deref(), Some("scope-old"));
            assert_eq!(account_id.as_deref(), Some("acct-old")); // prior reused
        }
        _ => panic!("expected OAuth2 cred"),
    }
}

#[test]
fn parse_callback_validates_state_and_extracts_code() {
    let cb = parse_callback("code=abc&state=xyz", "xyz").unwrap();
    assert_eq!(cb.code, "abc");
    assert_eq!(cb.state, "xyz");
}

#[test]
fn parse_callback_rejects_csrf_denied_and_missing_fields() {
    // CSRF: returned state must byte-equal the expected one.
    match parse_callback("code=abc&state=evil", "xyz").unwrap_err() {
        AuthError::Fatal(m) => assert!(m.contains("CSRF")),
        other => panic!("expected Fatal, got {other:?}"),
    }
    // The user declined.
    assert!(matches!(
        parse_callback("error=access_denied", "xyz").unwrap_err(),
        AuthError::Fatal(m) if m.contains("denied")
    ));
    // Missing code / state.
    assert!(matches!(
        parse_callback("state=xyz", "xyz").unwrap_err(),
        AuthError::Fatal(m) if m.contains("code")
    ));
    assert!(matches!(
        parse_callback("code=abc", "xyz").unwrap_err(),
        AuthError::Fatal(m) if m.contains("state")
    ));
}

#[test]
fn parse_callback_percent_decodes_every_branch() {
    // %2a (lower hex) → '*', %2F (upper hex) → '/', '+' → space, plain 'x',
    // invalid %zz → literal, trailing '%' → literal: exercises the whole codec.
    let cb = parse_callback("code=%2a%2F+x%zz%&state=ok", "ok").unwrap();
    assert_eq!(cb.code, "*/ x%zz%");
}

#[test]
fn parse_callback_ignores_unknown_and_bare_params() {
    // `iss=x` is an unknown param (ignored); `extra` is a bare key (no `=`).
    let cb = parse_callback("iss=x&extra&code=abc&state=ok", "ok").unwrap();
    assert_eq!(cb.code, "abc");
    assert_eq!(cb.state, "ok");
}

#[test]
fn query_from_request_line_extracts_the_query_or_none() {
    assert_eq!(
        query_from_request_line("GET /callback?code=x&state=y HTTP/1.1").as_deref(),
        Some("code=x&state=y")
    );
    // No query.
    assert_eq!(query_from_request_line("GET /callback HTTP/1.1"), None);
    // Malformed: no request-target token.
    assert_eq!(query_from_request_line("GET"), None);
}
