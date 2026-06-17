//! The five pure OAuth functions (auth §7.5, §6.1, §8): `is_expired`, the PKCE-S256
//! `build_authorize_url`, the one-builder `build_token_exchange_request` over
//! `Grant`, `parse_callback` (CSRF), and `parse_token_response` (absolute
//! `expires_at`) — all asserted from literals with zero IO and zero clock beyond an
//! explicit `now`, plus the loopback `query_from_request_line` and the percent codec
//! they share (exercised through `parse_callback`).

use brazen::{
    build_authorize_url, build_token_exchange_request, is_expired, parse_callback,
    parse_token_response, query_from_request_line, AuthError, Grant, OAuthConfig, Pkce,
    RedirectSpec, Secret, TokenResponse,
};

fn cfg() -> OAuthConfig {
    OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: Some("https://auth.example/device".into()),
        client_id: "cid".into(),
        scope: Some("read write".into()),
        beta_headers: vec![],
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
    }
}

#[test]
fn is_expired_is_a_query_with_a_stale_boundary() {
    // fresh: now + SKEW (60) < expires_at.
    assert!(!is_expired(1000, 900));
    // boundary now == expires_at - SKEW is STALE (the `>=`).
    assert!(is_expired(1000, 940));
    // well past.
    assert!(is_expired(1000, 5000));
}

#[test]
fn pkce_derive_matches_the_rfc7636_vector() {
    // RFC 7636 Appendix B: the canonical verifier → S256 challenge.
    let pkce = Pkce::derive("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(
        pkce.challenge,
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert_eq!(pkce.verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
}

#[test]
fn authorize_url_is_pkce_s256_with_scope() {
    let pkce = Pkce {
        verifier: "v".into(),
        challenge: "CHAL".into(),
    };
    let url = build_authorize_url(&cfg(), &pkce, "xyz", "http://127.0.0.1:8080/callback");
    assert_eq!(
        url,
        "https://auth.example/authorize?response_type=code&client_id=cid\
         &redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fcallback&state=xyz\
         &code_challenge=CHAL&code_challenge_method=S256&scope=read%20write"
    );
}

#[test]
fn authorize_url_omits_absent_scope() {
    let mut c = cfg();
    c.scope = None;
    let pkce = Pkce {
        verifier: "v".into(),
        challenge: "CHAL".into(),
    };
    let url = build_authorize_url(&c, &pkce, "s", "http://127.0.0.1:1/callback");
    assert!(!url.contains("scope="));
    assert!(url.ends_with("code_challenge_method=S256"));
}

#[test]
fn token_exchange_is_one_builder_over_three_grants() {
    let rt = Secret::new("rt-1");
    let grants = [
        (
            Grant::Refresh { refresh_token: &rt },
            "grant_type=refresh_token&refresh_token=rt-1&client_id=cid",
        ),
        (
            Grant::AuthCode {
                code: "the-code",
                verifier: "the-verifier",
                redirect_uri: "http://127.0.0.1:9/callback",
            },
            "grant_type=authorization_code&code=the-code\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcallback\
             &code_verifier=the-verifier&client_id=cid",
        ),
        (
            Grant::Device {
                device_code: "dev-123",
            },
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
             &device_code=dev-123&client_id=cid",
        ),
    ];
    for (grant, body) in grants {
        let wire = build_token_exchange_request(&cfg(), grant);
        // Same URL + content-type across all three (the one-builder proof).
        assert_eq!(wire.url, "https://auth.example/token");
        assert_eq!(
            wire.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(String::from_utf8_lossy(&wire.body), body);
    }
}

#[test]
fn token_response_success_sets_absolute_expires_at() {
    let body = br#"{"access_token":"at","refresh_token":"rt","expires_in":3600,"scope":"read"}"#;
    let tok = parse_token_response(body, 1_000).unwrap();
    assert_eq!(tok.access_token.expose(), "at");
    assert_eq!(tok.refresh_token.as_ref().map(Secret::expose), Some("rt"));
    assert_eq!(tok.expires_at, 4_600); // now + expires_in, ABSOLUTE
    assert_eq!(tok.scope.as_deref(), Some("read"));
}

#[test]
fn token_response_minimal_omits_refresh_scope_and_defaults_expiry() {
    let tok = parse_token_response(br#"{"access_token":"at"}"#, 50).unwrap();
    assert_eq!(tok.refresh_token, None);
    assert_eq!(tok.scope, None);
    assert_eq!(tok.expires_at, 50);
}

/// Build a (signature-less, unverified) JWT `hdr.payload.sig` whose payload is the
/// given JSON — the wire shape `jwt_exp`/`jwt_account_id` read (auth §10.3, §10.4).
fn jwt(payload: serde_json::Value) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("hdr.{p}.sig")
}

#[test]
fn token_response_expiry_falls_back_to_the_jwt_exp_when_expires_in_absent() {
    // OpenAI returns NO `expires_in`; the access token's JWT `exp` is the absolute
    // expiry (auth §10.3) — used verbatim, NOT `now + exp`.
    let access = jwt(serde_json::json!({ "exp": 9_999 }));
    let body = format!(r#"{{"access_token":"{access}"}}"#);
    let tok = parse_token_response(body.as_bytes(), 1_000).unwrap();
    assert_eq!(tok.expires_at, 9_999);
    assert_eq!(tok.account_id, None);
}

#[test]
fn token_response_derives_account_id_from_the_id_token_claim() {
    let id = jwt(serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct-XYZ" }
    }));
    let body = format!(r#"{{"access_token":"at","expires_in":3600,"id_token":"{id}"}}"#);
    let tok = parse_token_response(body.as_bytes(), 0).unwrap();
    assert_eq!(tok.account_id.as_deref(), Some("acct-XYZ"));
    assert_eq!(tok.expires_at, 3_600); // expires_in still wins when present
}

#[test]
fn token_response_account_id_is_none_when_the_id_token_lacks_the_claim() {
    let id = jwt(serde_json::json!({ "sub": "user-1" }));
    let body = format!(r#"{{"access_token":"at","expires_in":1,"id_token":"{id}"}}"#);
    let tok = parse_token_response(body.as_bytes(), 0).unwrap();
    assert_eq!(tok.account_id, None);
}

#[test]
fn authorize_url_appends_extra_params_after_the_standard_set() {
    let mut c = cfg();
    c.authorize_params = vec![
        ("id_token_add_organizations".into(), "true".into()),
        ("originator".into(), "codex_cli_rs".into()),
    ];
    let pkce = Pkce {
        verifier: "v".into(),
        challenge: "CHAL".into(),
    };
    let url = build_authorize_url(&c, &pkce, "s", "http://localhost:1455/auth/callback");
    assert!(url
        .ends_with("&scope=read%20write&id_token_add_organizations=true&originator=codex_cli_rs"));
}

#[test]
fn token_response_error_bodies_map_to_poll_signals_and_fatals() {
    let pending = parse_token_response(br#"{"error":"authorization_pending"}"#, 0);
    assert_eq!(pending.unwrap_err(), AuthError::Pending);
    let slow = parse_token_response(br#"{"error":"slow_down"}"#, 0);
    assert_eq!(slow.unwrap_err(), AuthError::SlowDown);
    match parse_token_response(br#"{"error":"invalid_grant"}"#, 0).unwrap_err() {
        AuthError::Fatal(m) => assert_eq!(m, "invalid_grant"),
        other => panic!("expected Fatal, got {other:?}"),
    }
    // Neither access_token nor error.
    assert!(matches!(
        parse_token_response(br#"{"token_type":"Bearer"}"#, 0).unwrap_err(),
        AuthError::Fatal(_)
    ));
    // Not even JSON.
    assert!(matches!(
        parse_token_response(b"not json", 0).unwrap_err(),
        AuthError::Fatal(_)
    ));
}

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
