//! bl-2485 / bl-a661 / bl-77fa: the OAuth machinery is vendor-blind — every endpoint,
//! client id and scope is row DATA, never compiled-in policy (architecture.md §13
//! item 3, auth.md §7). Two halves are tested here. The GENERAL mechanism: a row an
//! operator pastes resolves cleanly, proven with neutral values that name no vendor.
//! And the ONE built-in oauth2 row (auth §10.5): `openai-chatgpt` ships so a bare
//! install has a reachable browser sign-in at all — placed so it moves no routing,
//! and accompanied by the guard that the ToS-restricted Anthropic subscription-OAuth
//! row still does NOT ship.

use crate::{
    defaults, fill_absent, lead_with_preamble, AuthId, CanonicalRequest, Content, HeaderScheme,
    PartialConfig, ProtocolId,
};

/// A neutral `oauth2` provider row, exercising every field the mechanism understands
/// without naming any vendor's endpoints, client id, or required system lead.
const RECIPE: &str = "\
[[provider]]
name = \"my-oauth\"
base_url = \"https://api.example/v1\"
protocol = \"anthropic_messages\"
auth = \"oauth2\"
api_header = { name = \"Authorization\", scheme = \"bearer\" }
beta_headers = [[\"x-example-version\", \"1\"]]
body_defaults = { max_tokens = 4096 }

[provider.oauth]
authorize_url = \"https://auth.example/authorize\"
token_url = \"https://auth.example/token\"
client_id = \"example-client-id\"
scope = \"example.read example.invoke\"
beta_headers = [[\"x-example-oauth-beta\", \"v1\"]]
system_preamble = \"You are operating in a sandboxed deployment.\"
";

const PREAMBLE: &str = "You are operating in a sandboxed deployment.";

fn select(provider: &str) -> PartialConfig {
    PartialConfig {
        provider: Some(provider.into()),
        ..Default::default()
    }
}

#[test]
fn exactly_one_oauth_row_ships_and_it_is_not_the_tos_restricted_one() {
    // The policy after the 2026-08-16 ruling (auth §7, §10.5): a browser sign-in must
    // be reachable on a bare install, so ONE oauth2 row ships — and only one, because
    // each is a vendor whose login policy the binary is then carrying.
    let d = defaults();
    let oauth2: Vec<&str> = d
        .providers
        .iter()
        .filter(|(_, p)| p.auth == Some(AuthId::OAuth2))
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(oauth2, ["openai-chatgpt"]);
    // bl-a661's invariant, unchanged and now stated as what it always was: the
    // Anthropic SUBSCRIPTION-OAuth path is not configured for the user, because that
    // vendor's terms restrict third-party use of the token. Neither the row nor a
    // turnkey recipe ships; the general mechanism above stays available.
    assert!(d.row("anthropic-oauth").is_none());
    assert_eq!(d.row("anthropic").unwrap().auth, Some(AuthId::ApiKey));
}

#[test]
fn the_shipped_oauth_row_carries_every_login_fact_as_data() {
    // Every field `bz --login --browser` reads off the row (auth §10.1–§10.5). Asserted
    // literally: the row IS the capability, so a silent edit to any one of these is a
    // broken sign-in, and no other test would catch it.
    let d = defaults();
    let row = d.row("openai-chatgpt").unwrap();
    assert_eq!(row.protocol, Some(ProtocolId::OpenAiResponses));
    assert_eq!(row.auth, Some(AuthId::OAuth2));
    assert_eq!(
        row.base_url.as_deref(),
        Some("https://chatgpt.com/backend-api/codex")
    );
    let hdr = row.api_header.as_ref().unwrap();
    assert_eq!(
        (hdr.name.as_str(), hdr.scheme),
        ("Authorization", HeaderScheme::Bearer)
    );

    let o = row.oauth.as_ref().unwrap();
    assert_eq!(o.authorize_url, "https://auth.openai.com/oauth/authorize");
    assert_eq!(o.token_url, "https://auth.openai.com/oauth/token");
    assert_eq!(o.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(
        o.scope.as_deref(),
        Some("openid profile email offline_access api.connectors.read api.connectors.invoke")
    );
    // The registered redirect is matched byte-exact by the AS, so all three parts are
    // data and all three must be the registered ones (auth §10.1).
    assert_eq!(o.redirect.host, "localhost");
    assert_eq!(o.redirect.port, Some(1455));
    assert_eq!(o.redirect.path, "/auth/callback");
    assert_eq!(
        o.authorize_params,
        [
            ("id_token_add_organizations".to_owned(), "true".to_owned()),
            ("codex_cli_simplified_flow".to_owned(), "true".to_owned()),
            ("originator".to_owned(), "codex_cli_rs".to_owned()),
        ]
    );
    assert_eq!(o.account_header.as_deref(), Some("ChatGPT-Account-ID"));
    assert_eq!(
        o.beta_headers,
        [("originator".to_owned(), "codex_cli_rs".to_owned())]
    );
    // No device_url: this vendor registered a loopback redirect only, so `bz --login`
    // against it needs `--browser` and the bare device flow is an honest 78 (auth §7.1).
    assert!(o.device_url.is_none());
    // No system_preamble: that is the Anthropic-OAuth mechanism, not this one.
    assert!(o.system_preamble.is_none());
}

#[test]
fn the_shipped_oauth_row_moves_neither_routing_nor_the_bare_default() {
    // It is LAST and claims no prefixes, so the two facts a new default row could
    // silently break are pinned: `bz "q"` with nothing named still reaches the head
    // row, and `-m gpt-…` still routes to the api-key `openai` row, not to this one.
    let d = defaults();
    assert!(d.row("openai-chatgpt").unwrap().model_prefixes.is_none());
    assert_eq!(
        d.providers.last().map(|(n, _)| n.as_str()),
        Some("openai-chatgpt")
    );
    let bare = PartialConfig::default()
        .or(defaults())
        .into_resolved(None, None)
        .unwrap();
    assert_eq!(bare.provider.name, "anthropic");
    let routed = PartialConfig::default()
        .or(defaults())
        .into_resolved(Some("gpt-5.4"), None)
        .unwrap();
    assert_eq!(routed.provider.name, "openai");
    // Reached the one way an alternate row is reached: by name.
    let named = select("openai-chatgpt")
        .or(defaults())
        .into_resolved(Some("gpt-5.4"), None)
        .unwrap();
    assert_eq!(named.provider.auth, AuthId::OAuth2);
    // The backend's request-body quirks ride the row too, so the first run after a
    // login streams instead of 400ing (auth §10.7).
    assert_eq!(
        named.provider.unsupported_body_keys,
        ["max_tokens", "temperature", "top_p"]
    );
}

/// The ball's whole point, asserted end to end: on an install with **no config file**
/// — the empty config below is the layer a first-run user has — `bz --login --browser`
/// completes and writes a credential. Before the row shipped this was exit 78
/// (`NoProvider`/no-oauth-block), so a stranger's only path to a credential was
/// hand-authoring an api key into a file they first had to know about.
#[test]
fn a_bare_install_can_complete_a_browser_login_with_no_config_file() {
    use crate::testing::{FakeBrowserLauncher, FakeCodeReceiver, FakePacer, MockTransport};
    use crate::tests::login_support::{run, Case};
    use crate::CredStore;

    let tx = MockTransport::ok(vec![br#"{"access_token":"at","refresh_token":"rt"}"#]);
    let browser = FakeBrowserLauncher::new();
    // The receiver's own port is ignored: the row pins 1455, and the pinned port is
    // what the AS matched when the client was registered.
    let receiver = FakeCodeReceiver::new(8080, "code=C&state=ST");
    let pacer = FakePacer::new();
    let (code, stderr, store) = run(Case {
        argv: &["--login", "--provider", "openai-chatgpt", "--browser"],
        config: "",
        tx: &tx,
        browser: &browser,
        receiver: &receiver,
        pacer: &pacer,
        now: 0,
        verifier: "v",
        state: "ST",
    });

    assert_eq!(code, 0);
    assert!(stderr.contains("logged in to `openai-chatgpt`"), "{stderr}");
    let url = &browser.opened()[0];
    assert!(
        url.starts_with("https://auth.openai.com/oauth/authorize?"),
        "{url}"
    );
    assert!(
        url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"),
        "{url}"
    );
    assert!(
        url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"),
        "{url}"
    );
    for p in [
        "id_token_add_organizations=true",
        "codex_cli_simplified_flow=true",
        "originator=codex_cli_rs",
    ] {
        assert!(url.contains(p), "{p} missing from {url}");
    }
    assert!(store.get("openai-chatgpt").is_some());
}

#[test]
fn a_configured_oauth2_row_resolves_cleanly_and_leads_with_its_preamble() {
    // The general mechanism: a pasted `oauth2` row resolves like any other, and every
    // auth-mode fact rides DATA — the bearer header, the auth-mode-dependent beta
    // headers, and a system_preamble prepended in resolution (auth §4.1) with no flag.
    let cfg = select("my-oauth")
        .or(crate::parse_config(RECIPE).unwrap())
        .or(defaults())
        .into_resolved(Some("some-model"), None)
        .unwrap();

    assert_eq!(cfg.provider.auth, AuthId::OAuth2);
    assert_eq!(cfg.provider.protocol, ProtocolId::AnthropicMessages);
    let hdr = cfg.provider.api_header.as_ref().unwrap();
    assert_eq!(hdr.name, "Authorization");
    assert_eq!(hdr.scheme, HeaderScheme::Bearer);
    assert_eq!(cfg.max_tokens, Some(4096)); // row body_defaults floor folds in

    let oauth = cfg.provider.oauth.as_ref().unwrap();
    assert_eq!(oauth.client_id, "example-client-id");
    assert_eq!(oauth.scope.as_deref(), Some("example.read example.invoke"));
    // auth-mode-DEPENDENT headers live on the oauth block, applied only under OAuth.
    assert_eq!(
        oauth.beta_headers,
        vec![("x-example-oauth-beta".into(), "v1".into())]
    );

    let mut req = CanonicalRequest::default();
    fill_absent(&mut req, &cfg);
    lead_with_preamble(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text(PREAMBLE.into())]));
}

#[test]
fn a_configured_alternate_row_claims_no_prefix_so_it_never_hijacks_routing() {
    // A pasted alternate row sets no model_prefixes, so `-m claude-…` with no --provider
    // still routes to the built-in api-key `anthropic` row — never an ambiguity (78).
    let cfg = crate::parse_config(RECIPE)
        .unwrap()
        .or(defaults())
        .into_resolved(Some("claude-haiku-4-5-20251001"), None)
        .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.provider.auth, AuthId::ApiKey);
}
