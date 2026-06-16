//! The dispatch seam (arch §4.4): `Registry` maps a `ProtocolId`/`AuthId` to a
//! shared `&'static dyn` impl. The pipeline looks up here — it NEVER matches a
//! vendor name. This is the one place that knows specific impls; adding a protocol
//! or auth is ONE insert into `builtin`. The concrete impls plug in via their own
//! tasks, so the skeleton starts empty and an unregistered id fails closed.

use std::collections::HashMap;

use crate::auth::{Auth, OAuth2Auth, StaticSecretAuth};
use crate::config::provider::{AuthId, ProtocolId};
use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::Protocol;

/// The protocol/auth dispatch tables (arch §4.4). Holds `&'static dyn` impls so
/// one instance is shared across every row of a given id.
pub struct Registry {
    protocols: HashMap<ProtocolId, &'static dyn Protocol>,
    auths: HashMap<AuthId, &'static dyn Auth>,
}

impl Registry {
    /// The built-in dispatch tables. Each protocol/auth task adds ONE insert here.
    /// All three auth ids ship: `api_key` and `bearer` map to the SAME
    /// `StaticSecretAuth` (two names, one impl — auth §3.1), `oauth2` to
    /// `OAuth2Auth`. `anthropic_messages` and `openai_chat` are registered; an
    /// unregistered id still fails closed.
    pub fn builtin() -> Self {
        let mut protocols: HashMap<ProtocolId, &'static dyn Protocol> = HashMap::new();
        protocols.insert(ProtocolId::AnthropicMessages, &AnthropicMessages);
        protocols.insert(ProtocolId::OpenAiChat, &OpenAiChat);
        let mut auths: HashMap<AuthId, &'static dyn Auth> = HashMap::new();
        auths.insert(AuthId::ApiKey, &StaticSecretAuth);
        auths.insert(AuthId::Bearer, &StaticSecretAuth);
        auths.insert(AuthId::OAuth2, &OAuth2Auth);
        Registry { protocols, auths }
    }

    /// Look up the protocol impl for a resolved row's `ProtocolId` — a map lookup,
    /// never a `match` on a vendor name.
    pub fn protocol(&self, id: ProtocolId) -> Option<&'static dyn Protocol> {
        self.protocols.get(&id).copied()
    }

    /// Look up the auth impl for a resolved row's `AuthId`.
    pub fn auth(&self, id: AuthId) -> Option<&'static dyn Auth> {
        self.auths.get(&id).copied()
    }
}
