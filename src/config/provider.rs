//! Provider rows as DATA (arch §4.2) and the closed protocol/auth vocabularies
//! the registry keys on (arch §4.4). A provider is a row, not a trait impl;
//! `ProtocolId`/`AuthId` are typo-checked config names AND registry keys, never a
//! `match` site. `HeaderSpec` dissolves "x-api-key vs Authorization: Bearer vs
//! x-goog-api-key" into one `(name, scheme)` pair so auth needs no vendor branch
//! (auth §2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::auth::OAuthConfig;
use crate::store::AmbientSpec;

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
    #[serde(rename = "claude_code")]
    ClaudeCode,
}

/// Which auth model a provider uses (arch §4.2, §4.4). A registry key. `ApiKey`
/// and `Bearer` differ only in `HeaderScheme`; both ship, plus `OAuth2` and `None`.
/// `None` is a keyless row (e.g. local Ollama): no credential is read and no auth
/// header is written, so it carries no `api_header` — a resolve invariant mirroring
/// the way an `OAuth2` row carries an `oauth` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthId {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "oauth2")]
    OAuth2,
    #[serde(rename = "none")]
    None,
}

/// The `[provider.models]` per-row model-discovery override (config §4.4,
/// model-discovery §3.2): the `bz --list-models` GET's path/query and the response
/// list keys, OVERRIDING the protocol's default `ModelsShape` (§3.1). Every key is
/// optional — an omitted one inherits the protocol default — and the whole block is
/// optional (absent ⇒ pure protocol default). `deny_unknown_fields` makes a typo'd
/// key a `MalformedFile` (config §2.3), like `oauth`. `strip` is NOT here: it is
/// protocol-only (Google's leading `models/`), never row-overridable. `query` mirrors
/// `authorize_params` (a `Vec<(k, v)>` URL-encoded by the same codec, auth §7.4); the
/// `skip_serializing_if` keeps an omitted key out of a `--dump-config` round-trip and
/// off the TOML serializer's no-`None` path.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_key: Option<String>,
    /// The OPTIONAL metadata key paths over the protocol default (model-discovery §3.2):
    /// the per-entry fields the decoder lifts into `Model.context_window` /
    /// `max_output_tokens` / `display_name`. A row names one to pull a fact its list
    /// serves under a non-default key (e.g. the Codex `/models` slug shape carries
    /// `context_window`, so `context_key = "context_window"`). Omitted ⇒ the protocol
    /// default (`""` for the dialects that serve nothing ⇒ the field stays `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name_key: Option<String>,
}

/// A resolved provider row (arch §4.2). Pure data: `name` is a table key never
/// matched on in the pipeline; `protocol`/`auth` are registry keys; `model_aliases`
/// drives the computed alias→wire-id lookup. Sparse user/file rows fold onto the
/// embedded defaults before a complete `Provider` is resolved (config §3.2).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Provider {
    pub name: String,
    /// The HTTP host, or `""` on an exec-transport row (claude-code spec §7.1):
    /// `exec` substitutes for it, and the exec transport never reads a URL — the
    /// empty-set path, not a special case.
    pub base_url: String,
    /// The subprocess program for an exec-transport dialect (claude-code spec §7.1):
    /// a name resolved on `PATH` or an absolute path, carried onto
    /// `ProviderCtx.exec`. Unread on HTTP-dialect rows (like `ambient`); a
    /// `claude_code` row without it fails at encode with a `Config` error.
    #[serde(default)]
    pub exec: Option<String>,
    pub protocol: ProtocolId,
    pub auth: AuthId,
    /// The auth header to write, present for every keyed row and absent exactly when
    /// `auth = "none"` — resolution pairs the two or fails (`IncompleteProvider`,
    /// →78), so a keyed row's `api_header.is_some()` is a resolve invariant.
    #[serde(default)]
    pub api_header: Option<HeaderSpec>,
    #[serde(default)]
    pub beta_headers: Vec<(String, String)>,
    /// Ordered query pairs appended to generation POST URLs only (config §4.3.1).
    /// The protocol still owns the path; the shared encoded/raw request tail adds
    /// these with the generic query codec. Empty leaves every existing URL unchanged.
    #[serde(default)]
    pub generation_query: Vec<(String, String)>,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, String>,
    /// Canonical request-body fields this backend cannot accept — the inverse of
    /// `body_defaults` (config §4.1): `fill_absent`'s sibling `strip_unsupported`
    /// drops each from the request whatever its source, so the encoder never emits
    /// it. Keys name CANONICAL fields (`max_tokens`, not the wire `max_output_tokens`),
    /// so the canonical→wire rename stays owned by `encode`. Empty for every backend
    /// that takes the standard params; the Codex row pins the three it 400s on.
    #[serde(default)]
    pub unsupported_body_keys: Vec<String>,
    /// The `[provider.models]` discovery override (config §4.4): the `--list-models`
    /// GET path/query and response list keys over the protocol default. `None` ⇒ the
    /// protocol default shape; carried verbatim (the verb overlays it per key). No
    /// resolve invariant — any protocol may carry or omit it.
    #[serde(default)]
    pub models: Option<ModelsOverride>,
    /// The auth-row `OAuthConfig` (auth §7.1), present exactly when `auth =
    /// "oauth2"` — resolution pairs the two or fails (`IncompleteProvider`, →78),
    /// so the `OAuth2` impl's `oauth.is_some()` is a resolve invariant (auth §1.3).
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
    /// The row's ambient credential source (auth §5.5), present when the row opts
    /// into zero-setup discovery (Claude Code's `~/.claude/.credentials.json`).
    /// `None` ⇒ the store is the only credential source. Unlike `oauth`/`api_header`
    /// it has no resolve invariant: any auth model may name an ambient fallback.
    #[serde(default)]
    pub ambient: Option<AmbientSpec>,
}
