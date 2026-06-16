//! Provider rows as DATA (arch §4.2) and the closed protocol/auth vocabularies
//! the registry keys on (arch §4.4). A provider is a row, not a trait impl;
//! `ProtocolId`/`AuthId` are typo-checked config names AND registry keys, never a
//! `match` site. `HeaderSpec` dissolves "x-api-key vs Authorization: Bearer vs
//! x-goog-api-key" into one `(name, scheme)` pair so auth needs no vendor branch
//! (auth §2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::auth::OAuthConfig;

/// The auth-header shape as data (auth §2): the only thing that names the auth
/// header, so `ApiKey`/`Bearer` share one data-driven header write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderSpec {
    pub name: String,
    pub scheme: HeaderScheme,
}

/// How the secret is written into the header value (auth §2). Two arms cover
/// every shipped wire convention; a `match` on it is value formatting, not vendor
/// dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderScheme {
    Raw,
    Bearer,
}

/// Which wire dialect a provider speaks (arch §4.2). A registry key, never a
/// `match` target; the explicit `rename` keeps the config spelling (`openai_chat`)
/// stable regardless of the Rust identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProtocolId {
    #[serde(rename = "openai_chat")]
    OpenAiChat,
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "google_generative_ai")]
    GoogleGenAi,
    #[serde(rename = "ollama_chat")]
    OllamaChat,
}

/// Which auth model a provider uses (arch §4.2, §4.4). A registry key. `ApiKey`
/// and `Bearer` differ only in `HeaderScheme`; both ship, plus `OAuth2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthId {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "oauth2")]
    OAuth2,
}

/// A resolved provider row (arch §4.2). Pure data: `name` is a table key never
/// matched on in the pipeline; `protocol`/`auth` are registry keys; `model_aliases`
/// drives the computed alias→wire-id lookup. Sparse user/file rows fold onto the
/// embedded defaults before a complete `Provider` is resolved (config §3.2).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub protocol: ProtocolId,
    pub auth: AuthId,
    pub api_header: HeaderSpec,
    #[serde(default)]
    pub beta_headers: Vec<(String, String)>,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub default_max_tokens: Option<u32>,
    /// The auth-row `OAuthConfig` (auth §7.1), present exactly when `auth =
    /// "oauth2"` — resolution pairs the two or fails (`IncompleteProvider`, →78),
    /// so the `OAuth2` impl's `oauth.is_some()` is a resolve invariant (auth §1.3).
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
}
