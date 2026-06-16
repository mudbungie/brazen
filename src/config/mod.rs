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
pub mod partial;
pub mod provider;
pub mod resolve;

pub use dump::{dump_config, redact};
pub use env::{config_path, partial_from_env, EnvSnapshot};
pub use errors::ConfigError;
pub use partial::{OutMode, PartialConfig, PartialProvider};
pub use resolve::{defaults, fill_absent, parse_config, read_config_file, resolve, ResolvedConfig};
