//! The credential store seam and the injected clock (auth §5, §5.4; arch §6.4,
//! §6.5). `Secret` redacts at the type level; a `Cred`'s variant IS the
//! token-kind discriminant (no `token_type` flag) and `expires_at` is absolute
//! (no `is_valid` flag — freshness is a query) — so this module stores only what
//! cannot be computed. No IO lives here: the XDG-backed file store and the system
//! clock are `bz`-side impls behind these traits; the in-memory `CredStore` and
//! `FakeClock` doubles live in `testing`.

use std::fmt;
use std::io;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

/// Persist and retrieve one secret bundle per provider (auth §5.2). Two methods
/// only: freshness is a query, refresh is `OAuth2::apply` using get+put, and
/// list/delete are control-plane. `get` returns `None` for a missing cred (the
/// no-creds path), never an error.
pub trait CredStore {
    fn get(&self, provider: &str) -> Option<Cred>;
    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()>;
}

/// The one injected time source in the data plane (auth §5.4): unix seconds. The
/// library never calls `SystemTime::now`; `bz` wires `SystemClock`, tests wire
/// `FakeClock`.
pub trait Clock {
    fn now(&self) -> u64;
}
