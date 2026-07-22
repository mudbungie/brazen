//! Operator-selectable transport (transport spec §4): the `[provider.transport]`
//! row block reaching EVERY request `bz` makes — generation, `--raw`,
//! `--list-models`, `--count-tokens` — through the one `stamp_transport` home, its
//! fold across config layers, and the `exec`/`transport` contradiction. Driven by
//! `MockTransport`, which records the whole `WireRequest`, `exec` included.

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::run_support::{empty_store, go, ok_basic, temp};
use crate::{Envelope, ExecSpec};

/// A row whose HTTP/TLS is owned by an operator program. `[provider.transport]`
/// attaches to the `[[provider]]` element above it (the spec §4.2 shape).
const RELAYED: &str = r#"
[[provider]]
name = "relayed"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "api_key"
api_header = { name = "x-api-key", scheme = "raw" }
beta_headers = [["anthropic-version", "2023-06-01"]]

  [provider.transport]
  program = "/opt/relay/http-relay"
  args = ["--profile", "reference"]
"#;

/// What every stamped wire must carry: the operator's program, its args verbatim,
/// and the HTTP envelope (the child is the TRANSPORT, not the provider).
fn delegate() -> Option<ExecSpec> {
    Some(ExecSpec {
        program: "/opt/relay/http-relay".into(),
        args: vec!["--profile".into(), "reference".into()],
        envelope: Envelope::Http,
    })
}

#[test]
fn the_generation_request_rides_the_selected_transport() {
    let cfg = temp(RELAYED);
    let tx = ok_basic();
    let o = go(
        &[
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "relayed",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "--max-tokens",
            "64",
            "hi",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let sent = tx.requests();
    assert_eq!(sent[0].exec, delegate());
    // The APPLICATION request is untouched by the choice of transport (spec §2.1):
    // same URL, same auth header, same body the built-in stack would have carried.
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
    assert_eq!(sent[0].header("x-api-key"), Some("sk"));
    // And the resolved silence budget still rides the same stamp (config §4.3).
    assert!(sent[0].timeouts.idle.is_some());
}

#[test]
fn the_raw_passthrough_rides_the_selected_transport() {
    let cfg = temp(RELAYED);
    let tx = ok_basic();
    let o = go(
        &[
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "relayed",
            "--api-key",
            "sk",
            "--raw",
        ],
        &[],
        br#"{"model":"claude-x","messages":[]}"#,
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].exec, delegate());
}

#[test]
fn the_list_models_get_rides_the_selected_transport() {
    let cfg = temp(RELAYED);
    let tx = MockTransport::ok(vec![br#"{"data":[{"id":"claude-x"}]}"#]);
    let o = crate::tests::list_models_support::go(
        &[
            "--list-models",
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "relayed",
            "--api-key",
            "sk",
        ],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].exec, delegate());
}

#[test]
fn the_count_tokens_request_rides_the_selected_transport() {
    let cfg = temp(RELAYED);
    let tx = MockTransport::ok(vec![br#"{"input_tokens":42}"#]);
    let o = crate::tests::count_support::go(
        &[
            "--count-tokens",
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "relayed",
            "--api-key",
            "sk",
        ],
        br#"{"model":"claude-x","messages":[{"role":"user","content":"hi"}]}"#,
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].exec, delegate());
}

#[test]
fn a_row_with_no_block_keeps_the_built_in_transport() {
    // "Ordinary provider rows remain unchanged" is structural, not a promise: no
    // block, no `exec` on the wire — the built-in ureq/rustls path, byte-identical.
    let tx = ok_basic();
    let o = go(
        &[
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].exec, None);
}

#[test]
fn the_oauth_refresh_control_request_inherits_the_delegate() {
    // The auth control request must not fall back to a different HTTP stack than the
    // data request it serves (transport §4.3): `OAuth2::apply` copies the whole
    // transport policy off the wire `run` stamped — the bounds AND the delegate.
    use crate::testing::FakeClock;
    use crate::{
        Auth, AuthCtx, Cred, HeaderScheme, HeaderSpec, OAuth2Auth, OAuthConfig, ProviderCtx,
        RedirectSpec, Secret, Timeouts, WireRequest,
    };

    let store = MemoryCredStore::with(
        "prov",
        Cred::OAuth2 {
            access_token: Secret::new("at-old"),
            refresh_token: Secret::new("rt-old"),
            expires_at: 100, // stale against the clock below ⇒ one refresh POST
            scope: None,
            account_id: None,
        },
    );
    let tx = MockTransport::ok(vec![br#"{"access_token":"at-new","expires_in":3600}"#]);
    let oauth = OAuthConfig {
        authorize_url: "https://auth.example/authorize".into(),
        token_url: "https://auth.example/token".into(),
        device_url: None,
        client_id: "cid".into(),
        scope: None,
        beta_headers: vec![],
        system_preamble: None,
        redirect: RedirectSpec::default(),
        authorize_params: vec![],
        account_header: None,
    };
    let header = HeaderSpec {
        name: "Authorization".into(),
        scheme: HeaderScheme::Bearer,
    };
    let mut wire = WireRequest::new("https://api.example/v1", b"{}".to_vec());
    wire.exec = delegate();
    wire.timeouts = Timeouts {
        connect: Some(7),
        response: Some(7),
        idle: Some(7),
    };
    OAuth2Auth
        .apply(
            &mut wire,
            &ProviderCtx {
                base_url: "https://api.example",
                model: "m",
                beta_headers: &[],
                exec: None,
            },
            &AuthCtx {
                store_key: "prov",
                inline_key: None,
                api_header: Some(&header),
                oauth: Some(&oauth),
                ambient: None,
            },
            &store,
            &FakeClock::new(1_000),
            &tx,
        )
        .unwrap();
    let refresh = &tx.requests()[0];
    assert_eq!(refresh.exec, delegate());
    assert_eq!(refresh.timeouts, wire.timeouts);
}
