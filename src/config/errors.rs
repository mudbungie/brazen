//! The `Config` (78) error set (config §7). Every contradiction is surfaced —
//! but two rows OWNING one model is not a contradiction and has no variant
//! here: the `providers` list is a priority list and its first owner wins
//! (arch §4.3, the retired `AmbiguousModel`). Each variant maps to one
//! `CanonicalError{ kind: Config }` and thus to exit 78 (arch §8) via the
//! single `From` below.

use crate::canonical::{CanonicalError, ErrorKind};

/// A configuration failure (arch §8 → exit 78). The variants refine the one
/// "bad config" row of the architecture into the specific surfaced cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Nothing to route to (config §7): no provider named and EITHER the routing
    /// model matches zero rows, OR there is no model AND the provider table is
    /// empty. The no-model-with-a-non-empty-table case is NOT here — it defaults
    /// to the first row (`route`). `--login` re-raises this when no provider is
    /// named (a credential write must name its target), bypassing that default.
    NoProvider,
    /// A provider was named but no row carries that name (config §7).
    UnknownProvider { name: String },
    /// The routed row is missing a required field after the fold (config §7).
    IncompleteProvider { name: String, field: &'static str },
    /// A value parses but is out of range or contradictory — a bad env scalar,
    /// `max_tokens = 0`, a NaN temperature, an unknown output mode (config §7).
    BadValue { key: String, detail: String },
    /// The config file exists but is not valid `PartialConfig` TOML — a typo'd
    /// key, a duplicate provider name, malformed syntax (config §2.3, §7).
    MalformedFile { detail: String },
    /// The `[ingress]` table cannot serve (ingress §6, §7): absent under
    /// `--serve`, missing its required `dialect`, an unknown `lossy_overrides`
    /// adaptation name, an unparseable `listen`, or a non-loopback `listen`
    /// without `token` (the refuse-to-start rule). Surfaced only on the
    /// serve/ingress paths: `--serve` resolves the table (`resolve_ingress`);
    /// `--in`, which needs no serve-complete table, still runs the
    /// override-name check (`validate_lossy_overrides`, ingress §4).
    Ingress { detail: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NoProvider => f.write_str(
                "no provider resolved: name one with --provider, or use a model a provider owns (a known alias or model-id family)",
            ),
            ConfigError::UnknownProvider { name } => write!(f, "unknown provider `{name}`"),
            ConfigError::IncompleteProvider { name, field } => {
                write!(f, "provider `{name}` is missing required field `{field}`")
            }
            ConfigError::BadValue { key, detail } => write!(f, "bad value for `{key}`: {detail}"),
            ConfigError::MalformedFile { detail } => write!(f, "malformed config: {detail}"),
            ConfigError::Ingress { detail } => write!(f, "ingress: {detail}"),
        }
    }
}

impl From<ConfigError> for CanonicalError {
    /// Every `ConfigError` is one `Config`-kind canonical error → exit 78.
    fn from(err: ConfigError) -> CanonicalError {
        CanonicalError {
            kind: ErrorKind::Config,
            message: err.to_string(),
            provider_detail: None,
            retry_after_seconds: None,
        }
    }
}
