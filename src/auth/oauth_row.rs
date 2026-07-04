//! The OAuth auth-row as DATA (auth §7.1, §10): everything a provider's OAuth
//! block declares — endpoints, `client_id`, scope, the auth-mode-dependent
//! headers/preamble, and the loopback redirect shape. Pure schema, no behavior:
//! the pure builders (`wire`), the flows, and `refresh` all take `&OAuthConfig`,
//! so no vendor policy is compiled into the core (severability: delete the row's
//! block, delete the capability).

use serde::{Deserialize, Serialize};

/// The OAuth auth-row as data (auth §7.1): endpoints, `client_id`, `scope`, and
/// the auth-mode-dependent `beta_headers` (e.g. `anthropic-beta: oauth-…`). The
/// provider NAME is deliberately absent — it lives once, as the row key /
/// `store_key`. The pure OAuth builders take `&OAuthConfig`, so no vendor policy
/// is compiled into the core. Like the `[[provider]]` row (config §2.3),
/// `deny_unknown_fields` makes a typo'd or MISPLACED key a `MalformedFile` rather
/// than a silent drop — a TOP-LEVEL row key (e.g. `unsupported_body_keys`) typed
/// under `[provider.oauth]` would otherwise vanish and the strip never fire (bl-9649).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthConfig {
    pub authorize_url: String,
    pub token_url: String,
    #[serde(default)]
    pub device_url: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub scope: Option<String>,
    /// Auth-mode-dependent STATIC headers (auth §4), e.g. `anthropic-beta: oauth-…`.
    #[serde(default)]
    pub beta_headers: Vec<(String, String)>,
    /// The system the request body must LEAD with under this auth mode (auth §4.1):
    /// a Claude-Code-scoped Anthropic OAuth token rejects a request whose system does
    /// not begin with `You are Claude Code, Anthropic's official CLI for Claude.` It is
    /// a BODY fact, so — unlike `beta_headers` — it cannot ride header-only `apply`;
    /// resolution prepends it to `req.system` before `encode` (`lead_with_preamble`).
    /// `None` ⇒ no preamble (the api-key analogue), leaving the system untouched.
    #[serde(default)]
    pub system_preamble: Option<String>,
    /// The loopback redirect as data (auth §10.1); default reproduces today's
    /// `http://127.0.0.1:{ephemeral}/callback`, so existing rows are unchanged.
    #[serde(default)]
    pub redirect: RedirectSpec,
    /// Extra authorize-URL params (auth §10.2), e.g. OpenAI's
    /// `id_token_add_organizations=true`. Default empty ⇒ URL byte-identical.
    #[serde(default)]
    pub authorize_params: Vec<(String, String)>,
    /// Header NAME for the credential's `account_id` (auth §10.4), e.g.
    /// `ChatGPT-Account-ID`. `None` ⇒ the header is not emitted.
    #[serde(default)]
    pub account_header: Option<String>,
}

/// The loopback redirect endpoint as data (auth §10.1). The default reproduces
/// today's literal — `127.0.0.1` (RFC 8252), an ephemeral port (`None` ⇒ `:0`), and
/// `/callback` — so deleting the block restores it (severability). A provider whose
/// registered redirect differs (OpenAI: `localhost:1455/auth/callback`) names it
/// here as data; the socket still binds the IPv4 loopback `127.0.0.1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedirectSpec {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_host() -> String {
    "127.0.0.1".to_owned()
}

fn default_path() -> String {
    "/callback".to_owned()
}

impl Default for RedirectSpec {
    fn default() -> Self {
        RedirectSpec {
            host: default_host(),
            port: None,
            path: default_path(),
        }
    }
}
