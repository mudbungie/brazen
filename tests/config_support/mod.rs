//! Shared helpers for the config-resolution integration tests (config §3, §7):
//! the production fold composition and the small request/file constructors. One
//! home so `config_resolve` (the fold + errors) and `config_route` (routing) need
//! no duplicated boilerplate. A subdirectory module (not its own test binary);
//! `#![allow(dead_code)]` because each split test crate uses only a subset.
#![allow(dead_code)]

use brazen::{CanonicalRequest, ConfigError, EnvSnapshot, PartialConfig, ResolvedConfig};

/// The production composition the binary runs (run/mod.rs): project the env,
/// fold `flags > env > file > defaults`, then route by the request model. The
/// request is not a fold operand — only its non-empty model routes (arch §4.3).
pub fn resolve(
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
    defaults: PartialConfig,
    req: Option<&CanonicalRequest>,
) -> Result<ResolvedConfig, ConfigError> {
    let env = brazen::partial_from_env(env)?;
    let req_model = req.map(|r| r.model.as_str()).filter(|m| !m.is_empty());
    flags.or(env).or(file).or(defaults).into_resolved(req_model)
}

pub fn no_env() -> EnvSnapshot {
    EnvSnapshot::default()
}

pub fn req(model: &str) -> CanonicalRequest {
    CanonicalRequest {
        model: model.into(),
        ..Default::default()
    }
}

pub fn file(toml: &str) -> PartialConfig {
    brazen::parse_config(toml).unwrap()
}

pub const ANTHROPIC_ROW: &str = "[[provider]]\nname = \"anthropic\"\nbase_url = \"https://api.anthropic.com\"\nprotocol = \"anthropic_messages\"\nauth = \"api_key\"\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\nbody_defaults = { max_tokens = 4096 }\nmodel_aliases = { sonnet = \"claude-3-5-sonnet\" }\n";

/// An `anthropic` row that opts INTO fuzzy matching via `model_prefixes` (the
/// shipped default's shape, plus the `sonnet` alias): a model it neither prefix-owns
/// nor aliases (e.g. `son`) is a partial SEED → `probe == true` (model-discovery §5.1).
pub const PREFIX_ROW: &str = "[[provider]]\nname = \"anthropic\"\nbase_url = \"https://api.anthropic.com\"\nprotocol = \"anthropic_messages\"\nauth = \"api_key\"\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\nbody_defaults = { max_tokens = 4096 }\nmodel_prefixes = [\"claude-\"]\nmodel_aliases = { sonnet = \"claude-3-5-sonnet\" }\n";

/// A row that opts OUT of fuzzy matching — NO `model_prefixes` (the shipped
/// `openai-responses`/`openai-chatgpt` shape): a PRESENT model is taken LITERALLY,
/// so `probe == false` (the bl-3989 regression guard, model-discovery §5.1).
pub const PREFIX_LESS_ROW: &str = "[[provider]]\nname = \"codex\"\nbase_url = \"https://chatgpt.com/backend-api/codex\"\nprotocol = \"openai_responses\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\n";
