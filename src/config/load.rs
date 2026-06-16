//! Config input (config §3.3, §3.5): turn a TOML string or an on-disk file into
//! a `PartialConfig`, and materialize the embedded defaults. The embedded table
//! travels the SAME `toml::from_str` path as a user file — "lowest precedence"
//! is just "last operand," no bootstrap case. A missing file is the fold identity
//! `default()`; a present-but-broken one is the only `MalformedFile` (78).

use std::path::Path;

use crate::canonical::CanonicalError;
use crate::config::errors::ConfigError;
use crate::config::partial::PartialConfig;

/// The embedded provider table (arch §4.2), parsed through the SAME path as a
/// user file — "lowest precedence" is just "last operand," no bootstrap case.
const DEFAULTS_TOML: &str = include_str!("../../data/defaults.toml");

/// Parse a config string into a `PartialConfig`, mapping any TOML/serde failure
/// (typo'd key, duplicate provider name, bad syntax) to `MalformedFile` — the
/// one place a present-but-broken file becomes an error, distinct from a missing
/// file's identity element (config §3.3, §7).
pub fn parse_config(toml_str: &str) -> Result<PartialConfig, ConfigError> {
    toml::from_str(toml_str).map_err(|e| ConfigError::MalformedFile {
        detail: e.to_string(),
    })
}

/// Read the config file at `path` into a `PartialConfig` (config §3.3): a present
/// file parses (malformed → 78), a missing/unreadable one is the fold identity
/// `default()`. The one sanctioned config-file read in the lib — `run` and `bz
/// login` both go through it, so the path-resolution and malformed handling have
/// one home. The path came from the injected env, so it is tempfile-testable.
pub fn read_config_file(path: &Path) -> Result<PartialConfig, CanonicalError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config(&text).map_err(CanonicalError::from),
        Err(_) => Ok(PartialConfig::default()),
    }
}

/// The embedded defaults as a `PartialConfig`. The one sanctioned `expect`: it
/// is on our own compile-time-committed constant, validated by a unit test, not
/// on external input (config §3.5).
pub fn defaults() -> PartialConfig {
    #[allow(clippy::expect_used)]
    parse_config(DEFAULTS_TOML).expect("embedded defaults.toml is a valid, committed constant")
}
