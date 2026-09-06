//! Ambient-discovery IO invariants (auth §5.5) — the `discover` half of the
//! XdgCredStore pins: a foreign credentials file is read and parsed (Claude Code's
//! ms → s), a missing/garbage file degrades to the no-creds `None`, and the
//! `expand_home_with` tilde expansion is a pure table. Sibling of the parent's
//! store round-trip/permission/atomicity pins.

use brazen::{AmbientFormat, AmbientSpec, Cred, CredStore};

use super::store_at;
use crate::native::creds::expand_home_with;

/// The Claude Code credentials shape (auth §5.5): `expiresAt` in MILLISECONDS.
const CLAUDE_CODE: &str = r#"{"claudeAiOauth":{"accessToken":"at-cc","refreshToken":"rt-cc","expiresAt":1781693903571,"scopes":["user:inference"]}}"#;

/// The Codex CLI shape (auth §5.5): the expiry is the access token's own `exp`
/// claim, so the token here is a signature-less JWT with payload `{"exp":9999}`.
const CODEX: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"examplehdr.eyJleHAiOjk5OTl9.examplesig","refresh_token":"rt-cx","account_id":"acct-7"}}"#;

#[test]
fn discover_reads_and_parses_an_ambient_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cc.json");
    std::fs::write(&path, CLAUDE_CODE).unwrap();
    // An absolute path passes `expand_home` through unchanged; the file is read and
    // handed to the pure `parse_ambient`, yielding the OAuth2 cred (ms → s).
    let store = store_at(tmp.path().join("credentials"));
    let spec = AmbientSpec {
        format: AmbientFormat::ClaudeCode,
        path: path.to_string_lossy().into_owned(),
    };
    match store.discover(&spec) {
        Some(Cred::OAuth2 {
            access_token,
            expires_at,
            ..
        }) => {
            assert_eq!(access_token.expose(), "at-cc");
            assert_eq!(expires_at, 1_781_693_903);
        }
        other => panic!("expected a discovered OAuth2 cred, got {other:?}"),
    }
}

#[test]
fn discover_reads_the_codex_file_through_the_same_file_arm() {
    // The second FILE format takes the same read (both are `~`-expanded paths); only
    // the pure parser differs. Read-only: nothing is written back to the source.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("auth.json");
    std::fs::write(&path, CODEX).unwrap();
    let before = std::fs::read(&path).unwrap();
    let store = store_at(tmp.path().join("credentials"));
    let spec = AmbientSpec {
        format: AmbientFormat::Codex,
        path: path.to_string_lossy().into_owned(),
    };
    match store.discover(&spec) {
        Some(Cred::OAuth2 {
            expires_at,
            account_id,
            ..
        }) => {
            assert_eq!(expires_at, 9_999);
            assert_eq!(account_id.as_deref(), Some("acct-7"));
        }
        other => panic!("expected a discovered OAuth2 cred, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the source is read-only"
    );
}

#[test]
fn discover_is_none_for_missing_or_malformed_files() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store_at(tmp.path().join("credentials"));
    let missing = AmbientSpec {
        format: AmbientFormat::ClaudeCode,
        path: tmp.path().join("nope.json").to_string_lossy().into_owned(),
    };
    assert_eq!(
        store.discover(&missing),
        None,
        "absent file is the no-creds path"
    );

    let bad = tmp.path().join("bad.json");
    std::fs::write(&bad, "not json").unwrap();
    let bad_spec = AmbientSpec {
        format: AmbientFormat::ClaudeCode,
        path: bad.to_string_lossy().into_owned(),
    };
    assert_eq!(
        store.discover(&bad_spec),
        None,
        "foreign/garbage file is None"
    );
}

#[test]
fn expand_home_substitutes_leading_tilde_and_passes_others_through() {
    // `~/x` joins the passed home; everything else is verbatim; `~/x` with no home is
    // None (discovery degrades to the no-creds path). The home is a parameter, so the
    // test touches no process-global `$HOME` (no env race with sibling bin tests).
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();
    let some_home = || Some(home_path.clone().into_os_string());
    assert_eq!(
        expand_home_with("~/.claude/.credentials.json", some_home()),
        Some(home_path.join(".claude/.credentials.json")),
    );
    assert_eq!(
        expand_home_with("/etc/creds.json", some_home()),
        Some(std::path::PathBuf::from("/etc/creds.json")),
    );
    assert_eq!(
        expand_home_with("~/x", None),
        None,
        "no home ⇒ no expansion"
    );
}
