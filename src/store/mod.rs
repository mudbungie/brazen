//! The credential store seam and the injected clock (auth §5, §5.4; arch §6.4,
//! §6.5). `Secret` redacts at the type level; a `Cred`'s variant IS the
//! token-kind discriminant (no `token_type` flag) and `expires_at` is absolute
//! (no `is_valid` flag — freshness is a query) — so this module stores only what
//! cannot be computed. No IO lives here: the XDG-backed file store and the system
//! clock are `bz`-side impls behind these traits; the in-memory `CredStore` and
//! `FakeClock` doubles live in `testing`. Ambient discovery (auth §5.5) splits the
//! same way — the pure `parse_ambient` (foreign bytes → `Cred`) is [`ambient`]'s,
//! its file read + `$HOME` expansion are the `bz` `discover` impl.

use std::fmt;
use std::io;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::canonical::Model;

mod ambient;
// The fail-open ingress replay stash (ingress.md §5): written by the --serve/--in
// masquerade shell after each response, recalled by the ingress decode join.
mod replay;

pub use ambient::{parse_ambient, AmbientFormat, AmbientSpec};
pub use replay::{content_key, ReplayStash};

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

/// The one injected time source in the data plane (auth §5.4): unix seconds. The
/// library never calls `SystemTime::now`; `bz` wires `SystemClock`, tests wire
/// `FakeClock`.
pub trait Clock {
    fn now(&self) -> u64;
}
