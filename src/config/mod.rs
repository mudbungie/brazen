//! Config schema, resolution & provider rows (the config spec). One
//! `PartialConfig` schema, four instances (flags/env/file/defaults), one
//! associative `Option::or` fold, then `into_resolved` — precedence is the
//! order of operands, data the reader can see, never branching code (config §3).
//! Pure throughout: env arrives as an injected [`EnvSnapshot`], the file as an
//! already-parsed `PartialConfig`; nothing here reads `std::env` or opens a file
//! (arch §6.5).

pub mod dump;
pub mod env;
pub mod errors;
pub mod load;
pub mod partial;
mod partial_de;
pub mod provider;
pub mod resolve;
pub mod resolved;

pub use dump::dump_config;
pub use env::{config_path, partial_from_env, EnvSnapshot};
pub use load::{defaults, read_config_file};
pub use partial::{OutMode, PartialConfig};
pub use resolved::{fill_absent, lead_with_preamble, strip_unsupported, ResolvedConfig};

// CLI-unreachable: each is produced/consumed internally via its leaf path; these root
// re-exports feed only the `#[cfg(test)]` lib prelude, so they are gated to stay out
// of the release build's surface and dead-code set (arch §9.8).
#[cfg(test)]
pub(crate) use dump::redact;
#[cfg(test)]
pub(crate) use errors::ConfigError;
#[cfg(test)]
pub(crate) use load::parse_config;
#[cfg(test)]
pub(crate) use partial::{LossyMode, PartialIngress, PartialProvider};
#[cfg(test)]
pub(crate) use resolve::IngressConfig;
