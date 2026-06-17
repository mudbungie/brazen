//! bl-2485: the Anthropic-via-Claude-subscription OAuth path ships as a documented
//! RECIPE, not a built-in row. architecture.md §13 item 3 / auth.md §7 deliberately
//! ship NO built-in OAuth row (Anthropic blocks third-party use of its OAuth tokens),
//! so the deliverable is the README recipe an operator pastes into their own config —
//! exactly like OpenAI "Sign in with ChatGPT" (auth §10.5). These tests tie that
//! recipe to its behavior: it resolves cleanly, supplies every fact the bearer/OAuth
//! one-liner used to hand-roll in `--config` (so none is left to the user), and does
//! not hijack `claude-…` routing for the api-key path.

use brazen::{
    defaults, fill_absent, lead_with_preamble, AmbientFormat, AuthId, CanonicalRequest, Content,
    HeaderScheme, PartialConfig, ProtocolId,
};

/// The `anthropic-oauth` recipe from the README ("Anthropic via a Claude
/// subscription (OAuth)"), as a user/file config layer. Kept in sync with the README
/// block: if the recipe changes, this test changes with it.
const RECIPE: &str = "\
[[provider]]
name = \"anthropic-oauth\"
base_url = \"https://api.anthropic.com\"
protocol = \"anthropic_messages\"
auth = \"oauth2\"
api_header = { name = \"Authorization\", scheme = \"bearer\" }
beta_headers = [[\"anthropic-version\", \"2023-06-01\"]]
body_defaults = { max_tokens = 4096 }
ambient = { format = \"claude_code\", path = \"~/.claude/.credentials.json\" }

[provider.oauth]
authorize_url = \"https://claude.ai/oauth/authorize\"
token_url = \"https://console.anthropic.com/v1/oauth/token\"
client_id = \"9d1c250a-e61b-44d9-88ed-5944d1962f5e\"
scope = \"org:create_api_key user:profile user:inference\"
beta_headers = [[\"anthropic-beta\", \"oauth-2025-04-20\"]]
system_preamble = \"You are Claude Code, Anthropic's official CLI for Claude.\"
";

const CLAUDE_CODE_LEAD: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

fn select(provider: &str) -> PartialConfig {
    PartialConfig {
        provider: Some(provider.into()),
        ..Default::default()
    }
}

#[test]
fn anthropic_oauth_is_not_a_built_in_row() {
    // The deliberate policy (architecture.md §13 item 3 / auth.md §7): the binary ships
    // no Anthropic OAuth row, so it bakes in no vendor login policy. Only the api-key
    // `anthropic` row is built in; the OAuth path is the README recipe above.
    assert!(!defaults().providers.contains_key("anthropic-oauth"));
}

#[test]
fn the_recipe_resolves_cleanly_and_auto_leads_with_the_preamble() {
    // The deliverable: every fact the bearer/OAuth path used to hand-roll in `--config`
    // is DATA on the pasted row, so `--provider anthropic-oauth -m claude-…` needs no
    // `--config`, no `--system`, and no `--api-key`.
    let cfg = select("anthropic-oauth")
        .or(brazen::parse_config(RECIPE).unwrap())
        .or(defaults())
        .into_resolved(Some("claude-haiku-4-5-20251001"))
        .unwrap();

    // Resolves to the OAuth2 + bearer + Anthropic-messages data plane.
    assert_eq!(cfg.provider.auth, AuthId::OAuth2);
    assert_eq!(cfg.provider.protocol, ProtocolId::AnthropicMessages);
    let hdr = cfg.provider.api_header.as_ref().unwrap();
    assert_eq!(hdr.name, "Authorization");
    assert_eq!(hdr.scheme, HeaderScheme::Bearer);
    // anthropic-version is auth-mode-INDEPENDENT (always sent by encode, auth §4).
    assert_eq!(
        cfg.provider.beta_headers,
        vec![("anthropic-version".into(), "2023-06-01".into())]
    );
    // Anthropic requires max_tokens; the row's body_defaults floor folds in.
    assert_eq!(cfg.max_tokens, Some(4096));

    let oauth = cfg.provider.oauth.as_ref().unwrap();
    assert_eq!(oauth.client_id, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(
        oauth.scope.as_deref(),
        Some("org:create_api_key user:profile user:inference")
    );
    // anthropic-beta: oauth-… is auth-mode-DEPENDENT — on the OAuth row, applied only
    // under OAuth (auth §4), never on the provider beta_headers.
    assert_eq!(
        oauth.beta_headers,
        vec![("anthropic-beta".into(), "oauth-2025-04-20".into())]
    );
    // Zero-setup: the row discovers the token Claude Code already wrote (auth §5.5).
    let amb = cfg.provider.ambient.as_ref().unwrap();
    assert_eq!(amb.format, AmbientFormat::ClaudeCode);
    assert_eq!(amb.path, "~/.claude/.credentials.json");

    // The Claude-Code system lead the OAuth token mandates is supplied with no --system:
    // sourced from the row's `system_preamble`, prepended in resolution (auth §4.1).
    let mut req = CanonicalRequest::default();
    fill_absent(&mut req, &cfg);
    lead_with_preamble(&mut req, &cfg);
    assert_eq!(
        req.system,
        Some(vec![Content::Text(CLAUDE_CODE_LEAD.into())])
    );
}

#[test]
fn the_recipe_claims_no_prefix_so_claude_models_still_route_to_the_api_key_row() {
    // The recipe sets NO model_prefixes (cf. openai-responses): the api-key `anthropic`
    // row keeps `claude-`, so `-m claude-…` with no --provider stays UNAMBIGUOUS (never
    // a 78). The OAuth row is opt-in via --provider; pasting the recipe never hijacks
    // routing for the api-key path.
    let cfg = brazen::parse_config(RECIPE)
        .unwrap()
        .or(defaults())
        .into_resolved(Some("claude-haiku-4-5-20251001"))
        .unwrap();
    assert_eq!(cfg.provider.name, "anthropic");
    assert_eq!(cfg.provider.auth, AuthId::ApiKey);
}
