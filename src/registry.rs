//! The dispatch seam (arch §4.4): `Registry` maps a `ProtocolId`/`AuthId` to a
//! shared `&'static dyn` impl. The pipeline looks up here — it NEVER matches a
//! vendor name. This is the one place that knows specific impls; adding a protocol
//! or auth is ONE insert into `builtin`. The concrete impls plug in via their own
//! tasks, so the skeleton starts empty and an unregistered id fails closed.

use std::collections::HashMap;

use crate::auth::Auth;
use crate::config::provider::{AuthId, ProtocolId};
use crate::protocol::Protocol;

/// The protocol/auth dispatch tables (arch §4.4). Holds `&'static dyn` impls so
/// one instance is shared across every row of a given id.
pub struct Registry {
    protocols: HashMap<ProtocolId, &'static dyn Protocol>,
    auths: HashMap<AuthId, &'static dyn Auth>,
}

impl Registry {
    /// The built-in dispatch tables. Each protocol/auth task adds ONE insert here
    /// (`protocols.insert(ProtocolId::OpenAiChat, &OpenAiChat)`, …); until then the
    /// tables are empty and a lookup of any id returns `None`.
    pub fn builtin() -> Self {
        Registry {
            protocols: HashMap::new(),
            auths: HashMap::new(),
        }
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
