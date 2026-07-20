//! `lead_with_preamble`: the auth-mode-required system lead (auth §4.1). An
//! `oauth2` row may mandate a leading system block; resolution prepends it AFTER
//! `fill_absent`, idempotently, and is a no-op without a preamble or on a non-oauth
//! row (the general path with empty input). Split from `config_fill` to keep both
//! under the repo's 300-line code-file cap.

use crate::{
    defaults, fill_absent, lead_with_preamble, CanonicalRequest, Content, PartialConfig,
    ResolvedConfig,
};

/// An `oauth2` row carrying an optional `system_preamble`. Mirrors the §4.1 /
/// §7.1 Anthropic-OAuth shape: a keyed row (needs `api_header`) with an `oauth`
/// block (needs `authorize_url`/`token_url`/`client_id`).
fn oauth_resolved(preamble: Option<&str>) -> ResolvedConfig {
    let pre = preamble
        .map(|p| format!("system_preamble = \"{p}\"\n"))
        .unwrap_or_default();
    let toml = format!(
        "[[provider]]\nname = \"anthropic-oauth\"\nbase_url = \"https://api.anthropic.com\"\nprotocol = \"anthropic_messages\"\nauth = \"oauth2\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\n\n[provider.oauth]\nauthorize_url = \"https://auth/authorize\"\ntoken_url = \"https://auth/token\"\nclient_id = \"cid\"\n{pre}"
    );
    PartialConfig {
        provider: Some("anthropic-oauth".into()),
        ..Default::default()
    }
    .or(crate::parse_config(&toml).unwrap())
    .or(defaults())
    .into_resolved(Some("m"), None)
    .unwrap()
}

const CC: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

#[test]
fn lead_with_preamble_prepends_the_oauth_rows_required_system() {
    let cfg = oauth_resolved(Some(CC));
    // No user/config system: the request leads with exactly the preamble.
    let mut bare = CanonicalRequest::default();
    fill_absent(&mut bare, &cfg);
    lead_with_preamble(&mut bare, &cfg);
    assert_eq!(bare.system, Some(vec![Content::Text(CC.into())]));
    // A user/config system follows the mandated lead (preamble LEADS, then theirs).
    let mut withsys = CanonicalRequest {
        system: Some(vec![Content::Text("be terse".into())]),
        ..Default::default()
    };
    fill_absent(&mut withsys, &cfg);
    lead_with_preamble(&mut withsys, &cfg);
    assert_eq!(
        withsys.system,
        Some(vec![
            Content::Text(CC.into()),
            Content::Text("be terse".into()),
        ])
    );
}

#[test]
fn lead_with_preamble_is_idempotent_when_the_system_already_leads() {
    // A re-fed transcript already leading with the preamble is left untouched —
    // the invariant is "leads with", not "prepend N times" (auth §4.1).
    let cfg = oauth_resolved(Some(CC));
    let mut req = CanonicalRequest {
        system: Some(vec![Content::Text(CC.into()), Content::Text("x".into())]),
        ..Default::default()
    };
    lead_with_preamble(&mut req, &cfg);
    assert_eq!(
        req.system,
        Some(vec![Content::Text(CC.into()), Content::Text("x".into())])
    );
}

#[test]
fn lead_with_preamble_is_a_noop_without_a_preamble() {
    // An oauth row that pins no `system_preamble`: the empty case leaves the system
    // exactly as `fill_absent` left it (incl. None) — the general path, not special.
    let cfg = oauth_resolved(None);
    let mut req = CanonicalRequest {
        system: Some(vec![Content::Text("keep".into())]),
        ..Default::default()
    };
    lead_with_preamble(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text("keep".into())]));
    let mut bare = CanonicalRequest::default();
    lead_with_preamble(&mut bare, &cfg);
    assert_eq!(bare.system, None); // None stays None — no empty `system` synthesized
}

#[test]
fn lead_with_preamble_is_a_noop_for_a_non_oauth_row() {
    // A standard `api_key` row carries no `oauth` block, so there is no preamble.
    let cfg = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    }
    .or(PartialConfig::default())
    .or(defaults())
    .into_resolved(Some("m"), None)
    .unwrap();
    let mut bare = CanonicalRequest::default();
    lead_with_preamble(&mut bare, &cfg);
    assert_eq!(bare.system, None);
}
