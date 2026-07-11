//! Ingress resolution (ingress §6, §7): lift the merged, sparse `[ingress]`
//! table into the [`IngressConfig`] the `--serve` listener consumes. Runs ONLY
//! when a serve/ingress path asks — an ordinary one-shot run never validates
//! the table (requiredness is to-serve, not to-parse), so deleting the table
//! deletes every ingress behavior (severability, ingress §6). Total and pure:
//! every failure is a `ConfigError::Ingress` → 78, no IO, no sniffing.

use std::collections::BTreeMap;

// Deliberately `core::net` (not the std path the purity test forbids):
// `SocketAddr` is pure DATA — a parsed ip:port, no sockets, no IO — so it
// belongs in the library; the listener ball's shim does the binding.
use core::net::SocketAddr;

use crate::config::errors::ConfigError;
use crate::config::partial::{LossyMode, PartialConfig, PartialIngress};
use crate::store::Secret;

/// The resolved bind default (ingress §6): loopback, so the zero-`listen`
/// table needs no token. A RESOLUTION default, not a `defaults.toml` row — an
/// `[ingress]` table in the embedded defaults would make every config serve.
const DEFAULT_LISTEN: &str = "127.0.0.1:4891";

/// Every declared lossy-adaptation name (ingress §4): the vocabulary
/// `lossy_overrides` keys must come from, so a typo'd override errors instead
/// of silently leaving the default in force. A const for now; each mapping
/// spec that introduces an adaptation adds its name here.
pub const KNOWN_ADAPTATIONS: &[&str] = &["thinking_replay", "document_url_drop"];

/// The resolved ingress config the `--serve` listener consumes (ingress §6,
/// §7): dialect named, bind address parsed, defaults applied, overrides
/// validated. Consumed by the listener ball; until it lands, only the test
/// suite drives this — hence the `dead_code` allowance on the impls below.
#[derive(Clone, Debug, PartialEq)]
pub struct IngressConfig {
    /// The client dialect — always explicit, never sniffed (ingress §2).
    pub dialect: String,
    /// The parsed bind address; the listener shim binds it verbatim.
    pub listen: SocketAddr,
    /// The optional bearer token (ingress §7): `None` only ever binds loopback.
    pub token: Option<Secret>,
    /// The global rung-3 policy (ingress §4), defaulted to `Adapt`.
    pub lossy: LossyMode,
    /// The validated per-case overrides; read via [`Self::lossy_for`].
    pub lossy_overrides: BTreeMap<String, LossyMode>,
}

impl IngressConfig {
    /// The per-case policy QUERY (ingress §4): the override for this
    /// adaptation name, else the global `lossy` default — policy has one
    /// home, the consumer never reads the map and the default separately.
    #[allow(dead_code)] // consumed by the --serve/--in listener ball (bl-6cb4)
    pub fn lossy_for(&self, adaptation: &str) -> LossyMode {
        self.lossy_overrides
            .get(adaptation)
            .copied()
            .unwrap_or(self.lossy)
    }
}

impl PartialConfig {
    /// Resolve the merged `[ingress]` table (ingress §6, §7) — the ingress
    /// mirror of `into_resolved`, called only by a serve/ingress path. No
    /// table is a `Config` error (78) naming it: `--serve` without `[ingress]`
    /// must refuse, not guess a dialect.
    #[allow(dead_code)] // consumed by the --serve/--in listener ball (bl-6cb4)
    pub fn resolve_ingress(&self) -> Result<IngressConfig, ConfigError> {
        let Some(table) = self.ingress.clone() else {
            return Err(err(
                "`--serve` needs an `[ingress]` table naming the client `dialect`; the config has none".into(),
            ));
        };
        table.into_resolved()
    }
}

impl PartialIngress {
    /// Lift the sparse table into the complete [`IngressConfig`] (ingress §6,
    /// §7): `dialect` required (the explicit no-sniffing selector), `listen`
    /// defaulted to loopback and parsed, every `lossy_overrides` key checked
    /// against [`KNOWN_ADAPTATIONS`], and the refuse-to-start rule — a
    /// non-loopback bind without `token` is a `Config` error (78), because an
    /// open listener wired to the operator's credentials must be a
    /// deliberate, authenticated act.
    fn into_resolved(self) -> Result<IngressConfig, ConfigError> {
        let Some(dialect) = self.dialect else {
            return Err(err(
                "`dialect` is required to serve — the ingress dialect is always named explicitly, never sniffed".into(),
            ));
        };
        for name in self.lossy_overrides.keys() {
            if !KNOWN_ADAPTATIONS.contains(&name.as_str()) {
                return Err(err(format!(
                    "unknown adaptation `{name}` in `lossy_overrides` (known: {})",
                    KNOWN_ADAPTATIONS.join(", ")
                )));
            }
        }
        let spelled = self.listen.as_deref().unwrap_or(DEFAULT_LISTEN);
        let listen: SocketAddr = spelled.parse().map_err(|_| {
            err(format!(
                "`listen` must be an `ip:port` socket address, got `{spelled}`"
            ))
        })?;
        if !listen.ip().is_loopback() && self.token.is_none() {
            return Err(err(format!(
                "refusing a non-loopback `listen` ({listen}) without `token` — set one, or bind loopback"
            )));
        }
        Ok(IngressConfig {
            dialect,
            listen,
            token: self.token,
            lossy: self.lossy.unwrap_or_default(),
            lossy_overrides: self.lossy_overrides,
        })
    }
}

fn err(detail: String) -> ConfigError {
    ConfigError::Ingress { detail }
}
