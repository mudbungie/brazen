//! The dispatch seam (arch §4.4): `Registry` maps a `ProtocolId`/`AuthId` to a
//! shared `&'static dyn` impl by a TOTAL match over the closed key-enum — never a
//! match on a vendor name (§4.4). The match is the single source of truth: adding
//! a protocol/auth variant fails to compile here until its arm is added, so the
//! id-set and the impl-set cannot drift and an "unregistered id" is unrepresentable
//! (no `Option`, no runtime panic).

use crate::auth::{Auth, NoAuth, OAuth2Auth, StaticSecretAuth};
use crate::config::provider::{AuthId, ProtocolId};
use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::claude_code::ClaudeCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::protocol::Protocol;

/// The protocol/auth dispatch seam (arch §4.4). A zero-field handle whose two
/// methods are total matches over the registry-key enums.
pub struct Registry;

impl Registry {
    /// The built-in dispatch tables. A new protocol/auth = ONE match arm + ONE
    /// module (and its `ProtocolId`/`AuthId` variant). Nothing else.
    pub fn builtin() -> Self {
        Registry
    }

    /// The protocol impl for a resolved row's `ProtocolId` — a TOTAL match on the
    /// closed key-enum (exhaustiveness is the registration guarantee), never a
    /// match on a vendor name.
    pub fn protocol(&self, id: ProtocolId) -> &'static dyn Protocol {
        match id {
            ProtocolId::OpenAiChat => &OpenAiChat,
            ProtocolId::AnthropicMessages => &AnthropicMessages,
            ProtocolId::OpenAiResponses => &OpenAiResponses,
            ProtocolId::GoogleGenAi => &GoogleGenAi,
            ProtocolId::OllamaChat => &OllamaChat,
            ProtocolId::ClaudeCode => &ClaudeCode,
        }
    }

    /// The auth impl for a resolved row's `AuthId`. `api_key` and `bearer` share
    /// one `StaticSecretAuth` (two names, one impl — auth §3.1); `oauth2` is
    /// `OAuth2Auth`; `none` is the keyless `NoAuth`.
    pub fn auth(&self, id: AuthId) -> &'static dyn Auth {
        match id {
            AuthId::ApiKey | AuthId::Bearer => &StaticSecretAuth,
            AuthId::OAuth2 => &OAuth2Auth,
            AuthId::None => &NoAuth,
        }
    }
}
