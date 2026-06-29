//! The credential store seam and the injected clock (auth §5, §5.4; arch §6.4,
//! §6.5). `Secret` redacts at the type level; a `Cred`'s variant IS the
//! token-kind discriminant (no `token_type` flag) and `expires_at` is absolute
//! (no `is_valid` flag — freshness is a query) — so this module stores only what
//! cannot be computed. No IO lives here: the XDG-backed file store and the system
//! clock are `bz`-side impls behind these traits; the in-memory `CredStore` and
//! `FakeClock` doubles live in `testing`. Ambient discovery (auth §5.5) splits the
//! same way — the pure `parse_ambient` (foreign bytes → `Cred`) is here, its file
//! read + `$HOME` expansion are the `bz` `discover` impl.

use std::fmt;
use std::io;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::canonical::Model;

/// A plaintext secret whose `Debug`/`Display` redact and whose only plaintext
/// reads are `expose()` (the single audited site) and `Serialize` (reached only
/// by `CredStore::put` writing the 0600 file) — auth §5.3.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a plaintext secret.
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The single audited plaintext read site (auth §5.3).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for Secret {
    /// Writes PLAINTEXT — only ever reached via `CredStore::put` serializing into
    /// the 0600 credential file (auth §5.3).
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Secret(String::deserialize(d)?))
    }
}

/// The stored secret bundle for one provider (auth §5.1). The variant IS the
/// token-kind discriminant; `expires_at` is ABSOLUTE unix-seconds; there is no
/// `is_valid` flag (freshness is the `now + SKEW >= expires_at` query) and no
/// provider name (the file path is the name).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cred {
    ApiKey {
        key: Secret,
    },
    Bearer {
        token: Secret,
    },
    OAuth2 {
        access_token: Secret,
        refresh_token: Secret,
        expires_at: u64,
        #[serde(default)]
        scope: Option<String>,
        /// A non-secret account id some providers bind to the credential and
        /// require echoed as a request header (OpenAI's `ChatGPT-Account-ID`,
        /// derived once at login from the id_token claim — auth §10.4). `None` for
        /// OAuth rows that carry no account id. Not a `Secret`: it is echoed in a
        /// header, not a credential.
        #[serde(default)]
        account_id: Option<String>,
    },
}

/// Persist and retrieve one secret bundle per provider (auth §5.2, §5.5). `get`
/// returns `None` for a missing cred (the no-creds path), never an error; refresh
/// is `OAuth2::apply` using get+put (freshness is a query); list/delete are
/// control-plane. `discover` is the third primitive (auth §5.5): read a *foreign*
/// credential source named by an [`AmbientSpec`] — a file (Claude Code's `~/.claude/…`)
/// or a process env var (a vendor key alias) — into a brazen `Cred`. The IO — a file
/// read with `$HOME` expansion, or an env read — lives in the `bz` impl; the format
/// parse is the pure [`parse_ambient`]; a store with no ambient backing returns `None`.
pub trait CredStore {
    fn get(&self, provider: &str) -> Option<Cred>;
    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()>;
    fn discover(&self, spec: &AmbientSpec) -> Option<Cred>;
}

/// The per-provider model-list cache (model-discovery §5.1) — filesystem state, so
/// like `CredStore` it lives behind an injected trait; the pure lib never touches the
/// disk. A SIBLING of `CredStore`, not folded into it: a secret and a regenerable
/// model list are different facts with different files. The `bz` bin backs it with
/// one JSON file per provider under `$XDG_CACHE_HOME/brazen/models/<provider>.json`
/// (the `{"models":[{id,default}]}` shape `list-models --json` emits, reused); the
/// in-memory double lives in [`testing`](crate::testing).
///
/// Regenerable: a miss — or an unreadable/corrupt/garbage file — is `None`, never an
/// error, so a cold or corrupt cache degrades to `select_model`'s verbatim path and
/// self-heals on the next `bz --list-models`. `put` has two callers — `--list-models`
/// (wholesale replace) and the generation data plane (learn-on-success append of the one
/// model a 2xx used, model-discovery §5.4) — and is atomic (temp + rename so a concurrent
/// reader never sees a half-written file) and best-effort: a write failure warns but does
/// not fail the request.
pub trait ModelCache {
    /// The cached list for `provider`, or `None` for no usable cache (the empty list).
    fn get(&self, provider: &str) -> Option<Vec<Model>>;
    /// Write `provider`'s cached list: `--list-models` REPLACES it wholesale; the data
    /// plane APPENDS one learned id (§5.4). Atomic + best-effort.
    fn put(&self, provider: &str, models: &[Model]);
}

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

/// The one injected time source in the data plane (auth §5.4): unix seconds. The
/// library never calls `SystemTime::now`; `bz` wires `SystemClock`, tests wire
/// `FakeClock`.
pub trait Clock {
    fn now(&self) -> u64;
}
