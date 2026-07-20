//! bl-2485 / bl-a661: the OAuth machinery is reachable by config but ships as NO
//! built-in row — the core bakes in no vendor login policy (architecture.md §13 item 3,
//! auth.md §7). These tests prove the GENERAL `auth = "oauth2"` mechanism an operator
//! configures resolves cleanly — with NEUTRAL, vendor-agnostic values, since brazen
//! ships no turnkey recipe for any specific (and especially ToS-restricted) provider.

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
fn no_oauth_row_ships_built_in() {
    // The deliberate policy: the binary configures no OAuth path for any vendor, so it
    // bakes in no login policy and facilitates no specific provider's flow. In
    // particular, no Anthropic subscription-OAuth row ships (ToS-restricted use).
    let d = defaults();
    assert!(d.row("anthropic-oauth").is_none());
    let any_oauth2 = d
        .providers
        .iter()
        .any(|(_, p)| p.auth == Some(AuthId::OAuth2));
    assert!(!any_oauth2, "no built-in row should use auth = oauth2");
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
