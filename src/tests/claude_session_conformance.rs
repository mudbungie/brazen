//! Offline application-wire conformance for the operator-owned Claude-session
//! recipe (architecture §9.7.1). This is deliberately not transport/TLS identity
//! and does not add a built-in provider: a temp row drives the generic raw path.

use std::collections::BTreeMap;

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::run_support::{go, temp};
use crate::{Cred, CredStore, Method, Secret};

const EXPECTED: &[u8] =
    include_bytes!("../../tests/fixtures/claude_code_2_1_217_application_request.json");
const BODY: &[u8] = include_bytes!("../../tests/fixtures/claude_session_request.json");
const TOOL_RESPONSE: &[u8] =
    include_bytes!("../../tests/fixtures/anthropic_messages_thinking_tools.sse");

const RECIPE: &str = r#"
[[provider]]
name = "claude-session-direct"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
generation_query = [["beta", "true"]]
ambient = { format = "claude_code", path = "~/.claude/.credentials.json" }
beta_headers = [
  ["accept", "application/json"],
  ["user-agent", "claude-cli/2.1.217 (external, sdk-cli)"],
  ["x-app", "cli"],
  ["anthropic-dangerous-direct-browser-access", "true"],
  ["anthropic-version", "2023-06-01"],
]

[provider.oauth]
authorize_url = "https://claude.ai/oauth/authorize"
token_url = "https://console.anthropic.com/v1/oauth/token"
client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
scope = "user:inference user:profile"
beta_headers = [["anthropic-beta", "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05,effort-2025-11-24,extended-cache-ttl-2025-04-11"]]
system_preamble = "You are Claude Code, Anthropic's official CLI for Claude."
"#;

#[test]
fn private_raw_recipe_matches_the_scrubbed_first_request_and_stops() {
    let expected: serde_json::Value = serde_json::from_slice(EXPECTED).unwrap();
    assert_eq!(
        expected["private_body_fixture"],
        "claude_session_request.json"
    );
    assert_eq!(
        expected["approved_substitutions"],
        serde_json::json!(["body.messages", "body.system", "body.tools"])
    );
    let cfg = temp(RECIPE);
    let store = MemoryCredStore::with_ambient(Cred::OAuth2 {
        access_token: Secret::new("fake-captured-token"),
        refresh_token: Secret::new("foreign-refresh-token"),
        expires_at: 4_000_000_000,
        scope: None,
        account_id: None,
    });
    let tx = MockTransport::ok(vec![TOOL_RESPONSE]);

    let out = go(
        &["--raw=in", "--json", "--provider", "claude-session-direct"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        BODY,
        &tx,
        &store,
    );
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains(r#""kind":{"tool_use""#));
    assert!(out.stdout.trim_end().ends_with(r#"{"type":"end"}"#));

    let requests = tx.requests();
    assert_eq!(
        requests.len(),
        1,
        "one generation; no retry or tool re-entry"
    );
    let actual = &requests[0];
    assert_eq!(actual.method, Method::Post);
    assert_eq!(expected["method"], "POST");
    assert_eq!(
        actual
            .url
            .strip_prefix("https://api.anthropic.com")
            .unwrap(),
        expected["target"].as_str().unwrap()
    );
    assert_eq!(actual.body, BODY, "raw request body bytes are caller-owned");
    let actual_body: serde_json::Value = serde_json::from_slice(&actual.body).unwrap();
    let mut normalized_reference = expected["body"].clone();
    for key in ["messages", "system", "tools"] {
        normalized_reference[key] = actual_body[key].clone();
    }
    assert_eq!(
        actual_body, normalized_reference,
        "undeclared reference-client body shape drift"
    );

    let mut headers: BTreeMap<String, String> = actual
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    headers.insert("authorization".into(), "<borrowed-oauth-token>".into());
    let expected_headers: BTreeMap<String, String> =
        serde_json::from_value(expected["headers"].clone()).unwrap();
    assert_eq!(headers, expected_headers);
    assert_eq!(store.get("claude-session-direct"), None);
}
