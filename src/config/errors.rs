//! The `Config` (78) error set (config §7). Every contradiction is surfaced,
//! never papered over with a silent pick — most pointedly `AmbiguousModel`,
//! which names every matching provider so the operator can disambiguate
//! (arch §4.3). Each variant maps to one `CanonicalError{ kind: Config }` and
//! thus to exit 78 (arch §8) via the single `From` below.

use crate::canonical::{CanonicalError, ErrorKind};

/// A configuration failure (arch §8 → exit 78). The variants refine the one
/// "bad config" row of the architecture into the specific surfaced cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// No provider named and the routing model matches zero rows (or there is
    /// no model at all): nothing to route to (config §7).
    NoProvider,
    /// A provider was named but no row carries that key (config §7).
    UnknownProvider { name: String },
    /// No provider named and the routing model matches two-or-more rows'
    /// `model_aliases` — ambiguity is surfaced, never silently picked (arch §4.3).
    AmbiguousModel {
        model: String,
        providers: Vec<String>,
    },
    /// The routed row is missing a required field after the fold (config §7).
    IncompleteProvider { name: String, field: &'static str },
    /// A value parses but is out of range or contradictory — a bad env scalar,
    /// `max_tokens = 0`, a NaN temperature, an unknown output mode (config §7).
    BadValue { key: String, detail: String },
    /// The config file exists but is not valid `PartialConfig` TOML — a typo'd
    /// key, a duplicate provider name, malformed syntax (config §2.3, §7).
    MalformedFile { detail: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NoProvider => f.write_str(
                "no provider resolved: name one with --provider, or use a model with a known alias",
            ),
            ConfigError::UnknownProvider { name } => write!(f, "unknown provider `{name}`"),
            ConfigError::AmbiguousModel { model, providers } => write!(
                f,
                "model `{model}` matches multiple providers ({}); disambiguate with --provider",
                providers.join(", ")
            ),
            ConfigError::IncompleteProvider { name, field } => {
                write!(f, "provider `{name}` is missing required field `{field}`")
            }
            ConfigError::BadValue { key, detail } => write!(f, "bad value for `{key}`: {detail}"),
            ConfigError::MalformedFile { detail } => write!(f, "malformed config: {detail}"),
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
        }
    }
}
