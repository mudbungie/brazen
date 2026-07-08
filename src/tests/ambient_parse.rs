//! The pure ambient-format parsers (auth §5.5): `parse_ambient` over each
//! `AmbientFormat` — every malformed ClaudeCode shape is `None` (never a panic),
//! the scope/expiry projections, and the ApiKeyEnv trim/reject rules. The
//! store-miss discovery flow that CONSUMES a parsed cred lives in
//! `ambient_discovery`; the real file read is the `bz` shim's.

use crate::{parse_ambient, AmbientFormat, Cred, Secret};

#[test]
fn parse_claude_code_rejects_each_malformed_shape() {
    // The pure ClaudeCode parser's no-creds paths (auth §5.5): every missing or
    // wrong-typed field is `None`, not a panic — pinned in-crate now that the parse is
    // also reached directly (the happy path lives in the bz shim's `discover` test).
    for bad in [
        &b"not json"[..],
        br#"{}"#,                                                       // no claudeAiOauth
        br#"{"claudeAiOauth":{}}"#,                                     // no accessToken
        br#"{"claudeAiOauth":{"accessToken":1}}"#,                      // accessToken not a string
        br#"{"claudeAiOauth":{"accessToken":"a"}}"#,                    // no refreshToken
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":1}}"#,   // refreshToken not a string
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#, // no expiresAt
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":"x"}}"#, // expiresAt not u64
    ] {
        assert_eq!(parse_ambient(AmbientFormat::ClaudeCode, bad), None);
    }
}

#[test]
fn parse_claude_code_scope_is_none_when_absent_or_empty() {
    // `scopes` absent OR present-but-empty both yield `scope: None` (the join's empty
    // case); `expiresAt` MILLISECONDS divide to absolute seconds.
    for bytes in [
        &br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":2000}}"#[..],
        br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":2000,"scopes":[]}}"#,
    ] {
        match parse_ambient(AmbientFormat::ClaudeCode, bytes) {
            Some(Cred::OAuth2 {
                scope, expires_at, ..
            }) => {
                assert_eq!(scope, None);
                assert_eq!(expires_at, 2);
            }
            other => panic!("expected an OAuth2 cred, got {other:?}"),
        }
    }
}

#[test]
fn parse_api_key_env_trims_and_rejects_empty_or_non_utf8() {
    // The env var's value IS the raw key — trimmed (a `$(cat keyfile)` newline); empty,
    // whitespace-only, or non-UTF-8 is the no-creds path, so a blank var writes no header.
    assert_eq!(
        parse_ambient(AmbientFormat::ApiKeyEnv, b"  sk-ant-xyz\n"),
        Some(Cred::ApiKey {
            key: Secret::new("sk-ant-xyz"),
        }),
    );
    assert_eq!(parse_ambient(AmbientFormat::ApiKeyEnv, b"   "), None);
    assert_eq!(parse_ambient(AmbientFormat::ApiKeyEnv, b""), None);
    assert_eq!(parse_ambient(AmbientFormat::ApiKeyEnv, &[0xff, 0xfe]), None);
}
