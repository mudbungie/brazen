//! The sparse `[ingress]` table (ingress §6): the one genuinely new config
//! surface the masquerade adds — a TOP-LEVEL sibling of `[[provider]]`,
//! `deny_unknown_fields` like a row (a typo'd key is a `MalformedFile`, config
//! §2.3). Every field is `Option` so the table folds through the same
//! `PartialConfig::or` ladder as everything else (config §3); a missing table is
//! the fold identity, exactly like a missing file. Semantics live in ingress.md
//! §4/§6/§7; the resolved half (validation, defaults) is `resolve::ingress`.
//! NO model routing lives here — an inbound model resolves through the existing
//! alias/prefix ladder (ingress §6), never a second table.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::store::Secret;

/// The rung-3 lossy-adaptation policy (ingress §3, §4): `adapt` takes the lossy
/// adaptation by default (runtime-visible, never silent); `reject` collapses
/// rung 3 to rung 4 — refuse at the edge instead. One enum for both the global
/// `lossy` default and each per-case `lossy_overrides` value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LossyMode {
    #[default]
    Adapt,
    Reject,
}

/// The sparse `[ingress]` table (ingress §6). Requiredness is NOT parse-time:
/// `dialect` is required only to serve, so the check belongs to resolution when
/// a serve/ingress path asks (`resolve_ingress`), never to `Deserialize`. The
/// `Serialize` half (for `--dump-config`, token elided via `expose()`) lives in
/// `config::dump` beside `PartialConfig`'s.
#[derive(Default, Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialIngress {
    /// The client dialect the listener decodes — the explicit no-sniffing
    /// selector (ingress §2, §6). Required to serve; `None` until resolution.
    pub dialect: Option<String>,
    /// The bind address as written (`ip:port`); resolution parses it and
    /// defaults `127.0.0.1:4891` (ingress §6). Non-loopback without `token`
    /// refuses to start (ingress §7) — checked at resolution, not here.
    pub listen: Option<String>,
    /// The optional bearer token; when set, a request without it gets the
    /// dialect's 401 (ingress §7). A `Secret`: redacted in `--dump-config`
    /// like every secret (config §6).
    pub token: Option<Secret>,
    /// The global rung-3 default (ingress §4); resolution defaults `adapt`.
    pub lossy: Option<LossyMode>,
    /// Per-case overrides keyed by adaptation NAME (ingress §4). An UNKNOWN
    /// name is a `Config` error (78) at resolution — a typo'd override must
    /// never silently leave the default in force. Merged per-key across
    /// layers, like `body_defaults` (config §3.2).
    #[serde(default)]
    pub lossy_overrides: BTreeMap<String, LossyMode>,
}

impl PartialIngress {
    /// The fold step, one level down (config §3.2): `self` outranks `other`
    /// per field; `lossy_overrides` merges per-key (higher-precedence key
    /// wins, a lower-only key survives) — the same shape as `body_defaults`.
    fn or(mut self, other: PartialIngress) -> PartialIngress {
        for (key, value) in other.lossy_overrides {
            self.lossy_overrides.entry(key).or_insert(value);
        }
        PartialIngress {
            dialect: self.dialect.or(other.dialect),
            listen: self.listen.or(other.listen),
            token: self.token.or(other.token),
            lossy: self.lossy.or(other.lossy),
            lossy_overrides: self.lossy_overrides,
        }
    }
}

/// Fold the optional table across layers (config §3): a table in both merges
/// per-field under [`PartialIngress::or`]; a table in one layer passes through;
/// absence is the identity — the same dissolve as the missing file (§3.3).
pub(crate) fn or_ingress(
    hi: Option<PartialIngress>,
    lo: Option<PartialIngress>,
) -> Option<PartialIngress> {
    match (hi, lo) {
        (Some(hi), Some(lo)) => Some(hi.or(lo)),
        (hi, lo) => hi.or(lo),
    }
}
