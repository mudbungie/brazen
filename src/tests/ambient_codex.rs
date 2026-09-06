//! The `codex` ambient format and the spent-store fall-through (auth §5.5, §6.2).
//!
//! Two halves. The pure parser: the Codex CLI's `~/.codex/auth.json` shape →
//! `Cred::OAuth2`, with the expiry read out of the access token's own `exp` claim
//! and the account id taken from the file (never an id_token re-parse). And the
//! source WALK: a stored credential whose refresh the authorization server refuses
//! is spent, not authoritative — the row's ambient block gets the same look it
//! would have got on a store miss, so a box whose Codex sign-in is live answers
//! instead of refusing forever. Both no-answer arms name the file.

use crate::testing::{FakeClock, MemoryCredStore, MockTransport};
use crate::{
    defaults, parse_ambient, AmbientFormat, AmbientSpec, Auth, AuthCtx, CanonicalError, Cred,
    CredStore, HeaderScheme, HeaderSpec, OAuth2Auth, OAuthConfig, PartialConfig, ProviderCtx,
    RedirectSpec, Secret, WireRequest,
};

/// The Codex source the built-in `openai-chatgpt` row names.
fn codex_spec() -> AmbientSpec {
    AmbientSpec {
        format: AmbientFormat::Codex,
        path: "~/.codex/auth.json".into(),
    }
}

/// A signature-less JWT whose payload is `{"exp": <exp>}` — the shape `jwt_exp`
/// reads. Not signed: brazen is not the audience and never verifies (auth §10.3).
fn access_token(exp: u64) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
    format!("examplehdr.{payload}.examplesig")
}

/// The Codex file's bytes with `tokens.access_token` expiring at `exp`.
fn codex_file(exp: u64) -> Vec<u8> {
    format!(
        r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{
             "id_token":"unused","access_token":"{}","refresh_token":"rt.1.example",
             "account_id":"acct-7"}},"last_refresh":"2026-08-30T00:38:08Z"}}"#,
        access_token(exp)
    )
    .into_bytes()
}

#[test]
fn parse_codex_reads_the_tokens_object_and_the_jwt_expiry() {
    // The whole projection in one assertion: both secrets, the account id taken from
    // the file's own field (auth §10.4 — no id_token re-parse), no scope (the file
    // records none), and `expires_at` ABSOLUTE from the access token's `exp` claim,
    // with no `now` added.
    assert_eq!(
        parse_ambient(AmbientFormat::Codex, &codex_file(1_788_914_288)),
        Some(Cred::OAuth2 {
            access_token: Secret::new(access_token(1_788_914_288)),
            refresh_token: Secret::new("rt.1.example"),
            expires_at: 1_788_914_288,
            scope: None,
            account_id: Some("acct-7".into()),
        }),
    );
}

#[test]
fn parse_codex_account_id_is_optional() {
    // A file with no `account_id` is not malformed — the row simply emits no
    // `ChatGPT-Account-ID` header, the same empty case Anthropic's OAuth row takes.
    let bytes = format!(
        r#"{{"tokens":{{"access_token":"{}","refresh_token":"r"}}}}"#,
        access_token(9_000)
    );
    match parse_ambient(AmbientFormat::Codex, bytes.as_bytes()) {
        Some(Cred::OAuth2 { account_id, .. }) => assert_eq!(account_id, None),
        other => panic!("expected an OAuth2 cred, got {other:?}"),
    }
}

#[test]
fn parse_codex_rejects_each_malformed_shape() {
    // Every missing or unreadable field is `None`, the no-creds path (never a panic).
    // The last two are the expiry: a `Cred` must carry an absolute instant, and an
    // access token whose `exp` cannot be read supplies none — the sibling
    // `last_refresh` records when the token was MINTED, so it cannot stand in.
    let opaque = br#"{"tokens":{"access_token":"opaque","refresh_token":"r"},
             "last_refresh":"2026-08-30T00:38:08Z"}"#;
    for bad in [
        &b"not json"[..],
        br#"{}"#,                                                // no tokens object
        br#"{"tokens":{}}"#,                                     // no access_token
        br#"{"tokens":{"access_token":7}}"#,                     // access_token not a string
        br#"{"tokens":{"access_token":"a"}}"#,                   // no refresh_token
        br#"{"tokens":{"access_token":"a","refresh_token":7}}"#, // refresh_token not a string
        &opaque[..],                                             // access_token is not a JWT
    ] {
        assert_eq!(parse_ambient(AmbientFormat::Codex, bad), None);
    }
}

#[test]
fn the_openai_chatgpt_default_row_names_the_codex_file() {
    // The capability is row DATA, not a vendor branch: deleting this one line from
    // `data/defaults.toml` deletes it, with no Rust change (severability).
    let cfg = PartialConfig {
        provider: Some("openai-chatgpt".into()),
        ..Default::default()
    }
    .or(defaults())
    .into_resolved(None, None)
    .unwrap();
    assert_eq!(cfg.provider.ambient, Some(codex_spec()));
}

fn oauth_cfg() -> OAuthConfig {
    OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device: None,
        client_id: "cid".into(),
        scope: None,
        beta_headers: vec![],
        system_preamble: None,
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: Some("ChatGPT-Account-ID".into()),
    }
}

fn oauth_cred(access: &str, expires_at: u64, account: Option<&str>) -> Cred {
    Cred::OAuth2 {
        access_token: Secret::new(access),
        refresh_token: Secret::new("rt"),
        expires_at,
        scope: None,
        account_id: account.map(str::to_owned),
    }
}

/// `OAuth2Auth::apply` against a row that names the Codex ambient source, at `now`,
/// with `tx` answering the refresh `POST`.
fn apply(
    store: &dyn CredStore,
    now: u64,
    tx: &MockTransport,
) -> Result<WireRequest, CanonicalError> {
    let header = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let beta: Vec<(&str, &str)> = Vec::new();
    let ctx = ProviderCtx {
        base_url: "https://api.example",
        model: "m",
        beta_headers: &beta,
        exec: None,
    };
    let cfg = oauth_cfg();
    let amb = codex_spec();
    let authc = AuthCtx {
        store_key: "prov",
        inline_key: None,
        api_header: Some(&header),
        oauth: Some(&cfg),
        ambient: Some(&amb),
    };
    let clock = FakeClock::new(now);
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    OAuth2Auth.apply(&mut wire, &ctx, &authc, store, &clock, tx)?;
    Ok(wire)
}

/// A token-endpoint answer the token parser cannot read — the refusal (auth §6.2).
fn refused() -> MockTransport {
    MockTransport::ok(vec![br#"{"error":"invalid_grant"}"#])
}

#[test]
fn a_spent_store_credential_falls_through_to_a_live_ambient_sign_in() {
    // The ball's box: a `bz --login` from weeks ago whose refresh token the vendor
    // rotated under the OTHER tool's logins, so brazen's refresh is refused for good
    // — while that tool is signed in right now. The refusal is about ONE credential,
    // not about the box, so the ambient source answers and the request goes out.
    let store = MemoryCredStore::with_both(
        "prov",
        oauth_cred("at-spent", 100, Some("acct-old")),
        oauth_cred("at-ambient", 10_000, Some("acct-7")),
    );
    let tx = refused();
    let wire = apply(&store, 100, &tx).unwrap();
    assert_eq!(wire.header("Authorization"), Some("Bearer at-ambient"));
    // The account id travels with the credential that answered, not with the spent one.
    assert_eq!(wire.header("ChatGPT-Account-ID"), Some("acct-7"));
    assert_eq!(
        tx.requests().len(),
        1,
        "the refusal was a real refresh attempt"
    );
    // Borrowed means read-only: the rotation is never adopted into brazen's store,
    // so the spent cred stands and the next process re-reads the owner's bytes.
    assert_eq!(
        store.get("prov"),
        Some(oauth_cred("at-spent", 100, Some("acct-old")))
    );
}

#[test]
fn a_spent_store_credential_with_a_stale_ambient_one_names_the_file() {
    // Neither source can answer: the ambient credential is borrowed state brazen may
    // not refresh either. The message names the file, because "re-run `bz --login`"
    // is the wrong instruction for a box whose other tool holds the live sign-in.
    let store = MemoryCredStore::with_both(
        "prov",
        oauth_cred("at-spent", 100, None),
        oauth_cred("at-ambient", 200, None),
    );
    let err = apply(&store, 1_000, &refused()).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(
        err.message
            .contains("the ambient credential at ~/.codex/auth.json is absent or expired"),
        "{}",
        err.message
    );
}

#[test]
fn a_spent_store_credential_with_no_ambient_file_names_the_file_too() {
    // The row names a source; the box has none (Codex never signed in). Same
    // sentence — "absent or expired" covers both, and the file is what to look at.
    let store = MemoryCredStore::with("prov", oauth_cred("at-spent", 100, None));
    let err = apply(&store, 1_000, &refused()).unwrap_err();
    assert_eq!(err.exit_code(), 77);
    assert!(
        err.message.contains("~/.codex/auth.json"),
        "{}",
        err.message
    );
}

#[test]
fn a_successful_refresh_still_wins_over_the_ambient_source() {
    // The fall-through is the REFUSAL's path only: an owned credential brazen can
    // still refresh is refreshed, persisted and used, with no discovery at all.
    let store = MemoryCredStore::with_both(
        "prov",
        oauth_cred("at-spent", 100, Some("acct-old")),
        oauth_cred("at-ambient", 10_000, Some("acct-7")),
    );
    let tx = MockTransport::ok(vec![br#"{"access_token":"at-new","expires_in":3600}"#]);
    let wire = apply(&store, 100, &tx).unwrap();
    assert_eq!(wire.header("Authorization"), Some("Bearer at-new"));
    assert_eq!(wire.header("ChatGPT-Account-ID"), Some("acct-old"));
}
