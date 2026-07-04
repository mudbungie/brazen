//! Ambient credential discovery (auth §5.5): a *foreign* credential source as row
//! DATA — the [`AmbientSpec`] locator + [`AmbientFormat`] parser selector — and the
//! pure [`parse_ambient`] that maps its bytes to a `Cred`. The IO (a file read with
//! `$HOME` expansion, an env read) lives in the `bz` `discover` impl; deleting the
//! row's `ambient` block deletes the capability (severability).

use serde::{Deserialize, Serialize};

use super::{Cred, Secret};

/// An ambient credential source as row DATA (auth §5.5): `path` is the source
/// LOCATOR the `bz` impl reads — a `~`/`$HOME`-expanded filesystem path for a file
/// format, an environment-variable NAME for `api_key_env` — and `format` selects the
/// pure parser that maps its bytes to a `Cred`. Neither lives in core code, so
/// deleting the row's `ambient` block deletes the capability (severability).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbientSpec {
    pub format: AmbientFormat,
    pub path: String,
}

/// Which foreign credential format an [`AmbientSpec`] names (auth §5.5). A closed
/// enum, not a JSON-pointer DSL: each shape needs a parser anyway, so one variant
/// per known source is less mechanism than a speculative mapping language. The
/// variant ALSO tells the `bz` `discover` impl where to read: a file (`ClaudeCode`)
/// or a process env var (`ApiKeyEnv`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientFormat {
    ClaudeCode,
    /// A process environment variable whose VALUE is a raw API key — the
    /// vendor-conventional alias (e.g. `ANTHROPIC_API_KEY`) as a ROW-SCOPED, store-miss
    /// ambient source, so a stray vendor key can reach only the row that names it and
    /// never shadows a stored cred (`AmbientSpec.path` is the variable name).
    ApiKeyEnv,
}

/// Map a foreign credential source's bytes to a brazen `Cred` (auth §5.5) — the pure
/// half of discovery, so the `bz` impl does only the IO. `None` for malformed or
/// incomplete input (the no-creds path, like `get`). The `claude_code` case reads
/// `claudeAiOauth` into a `Cred::OAuth2`: `expiresAt` is MILLISECONDS, divided to
/// absolute unix-seconds once here (the single home for that unit mismatch), and
/// `scopes` join into the `scope` string (`None` when empty); `account_id` is `None`
/// (Anthropic binds no account id). The `api_key_env` case is the env var's value as a
/// raw `Cred::ApiKey` (trimmed; empty/non-UTF-8 ⇒ `None`).
pub fn parse_ambient(format: AmbientFormat, bytes: &[u8]) -> Option<Cred> {
    match format {
        AmbientFormat::ClaudeCode => parse_claude_code(bytes),
        AmbientFormat::ApiKeyEnv => parse_api_key_env(bytes),
    }
}

/// The `api_key_env` parse: the source's bytes ARE the raw API key. Trim incidental
/// surrounding whitespace (a `$(cat keyfile)` trailing newline), and treat empty or
/// non-UTF-8 as `None` — the no-creds path, so an `ANTHROPIC_API_KEY=` never writes a
/// blank auth header.
fn parse_api_key_env(bytes: &[u8]) -> Option<Cred> {
    let key = std::str::from_utf8(bytes).ok()?.trim();
    (!key.is_empty()).then(|| Cred::ApiKey {
        key: Secret::new(key),
    })
}

fn parse_claude_code(bytes: &[u8]) -> Option<Cred> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let oauth = v.get("claudeAiOauth")?;
    let access_token = Secret::new(oauth.get("accessToken")?.as_str()?);
    let refresh_token = Secret::new(oauth.get("refreshToken")?.as_str()?);
    let expires_at = oauth.get("expiresAt")?.as_u64()? / 1000;
    let scope = oauth
        .get("scopes")
        .and_then(|s| s.as_array())
        .and_then(|a| {
            let joined: Vec<&str> = a.iter().filter_map(serde_json::Value::as_str).collect();
            (!joined.is_empty()).then(|| joined.join(" "))
        });
    Some(Cred::OAuth2 {
        access_token,
        refresh_token,
        expires_at,
        scope,
        account_id: None,
    })
}
