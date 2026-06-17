//! The pure ambient-credential parser (auth §5.5): `parse_ambient` maps a foreign
//! tool's credential-file bytes to a brazen `Cred`. The shipped `claude_code` case
//! reads `claudeAiOauth` into a `Cred::OAuth2` — the file IO + `$HOME` expansion are
//! the `bz` `discover` impl, so this drives the parse end to end from byte fixtures.

use brazen::{parse_ambient, AmbientFormat, Cred, Secret};

/// A well-formed Claude Code credentials file (the real shape, redacted values).
/// `expiresAt` is MILLISECONDS — `1_781_693_903_571` ms = `1_781_693_903` s.
const CLAUDE_CODE: &[u8] = br#"{
  "claudeAiOauth": {
    "accessToken": "at-live",
    "refreshToken": "rt-live",
    "expiresAt": 1781693903571,
    "scopes": ["org:create_api_key", "user:profile", "user:inference"],
    "subscriptionType": "max"
  }
}"#;

#[test]
fn claude_code_parses_to_oauth2_with_seconds_and_joined_scope() {
    let cred = parse_ambient(AmbientFormat::ClaudeCode, CLAUDE_CODE).expect("valid file parses");
    assert_eq!(
        cred,
        Cred::OAuth2 {
            access_token: Secret::new("at-live"),
            refresh_token: Secret::new("rt-live"),
            // milliseconds divided to absolute unix-seconds, once, in the parser.
            expires_at: 1_781_693_903,
            scope: Some("org:create_api_key user:profile user:inference".into()),
            // Anthropic binds no account id (the OpenAI-only header, §10.4).
            account_id: None,
        }
    );
}

#[test]
fn empty_scopes_array_yields_none_scope() {
    let bytes =
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":2000,"scopes":[]}}"#;
    match parse_ambient(AmbientFormat::ClaudeCode, bytes).unwrap() {
        Cred::OAuth2 {
            scope, expires_at, ..
        } => {
            assert_eq!(scope, None);
            assert_eq!(expires_at, 2); // 2000 ms → 2 s
        }
        _ => panic!("expected OAuth2"),
    }
}

#[test]
fn missing_scopes_key_yields_none_scope() {
    // `scopes` absent entirely (not just empty) is also the no-scope case.
    let bytes = br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1000}}"#;
    match parse_ambient(AmbientFormat::ClaudeCode, bytes).unwrap() {
        Cred::OAuth2 { scope, .. } => assert_eq!(scope, None),
        _ => panic!("expected OAuth2"),
    }
}

#[test]
fn malformed_or_foreign_files_are_none() {
    // Each is the no-creds path (like `get` on a miss), never an error.
    let cases: &[&[u8]] = &[
        b"not json at all",
        br#"{}"#,                                                       // no claudeAiOauth
        br#"{"claudeAiOauth":{}}"#,                                     // no fields
        br#"{"claudeAiOauth":{"accessToken":"a"}}"#,                    // missing refresh/expiry
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#, // missing expiry
        br#"{"claudeAiOauth":{"accessToken":1,"refreshToken":"r","expiresAt":1}}"#, // wrong type
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":"soon"}}"#, // expiry not a number
    ];
    for bytes in cases {
        assert_eq!(parse_ambient(AmbientFormat::ClaudeCode, bytes), None);
    }
}
