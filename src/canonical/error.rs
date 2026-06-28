//! Canonical error model (§3.3, §8): the `kind` taxonomy — open via an `Other`
//! catch-all per the §3.2 `v=1` forward-compat contract — the computed
//! `retryable` query, and the pure exit-code tables. No IO; every mapping is a
//! pure function of `kind`/`io::Error` so it is table-tested without a network.

use std::io;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A normalized error, carried in-band as `Event::Error` (§3.3). It stores only
/// what cannot be computed: `retryable` and the exit code are *queries* over
/// `kind`, never fields that could drift.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalError {
    pub kind: ErrorKind,
    pub message: String,
    /// The parsed upstream error body, verbatim, when one exists.
    #[serde(default)]
    pub provider_detail: Option<Value>,
}

/// The taxonomy every failure normalizes to (§3.3). `Provider` carries the HTTP
/// status so `retryable`/exit derive without a second table. `Other` is the
/// forward-compat escape hatch (§3.2 `v=1` contract): an error event carries no
/// `v` handshake, so a future kind cannot be version-gated — instead an
/// unrecognized snake_case `kind` decodes here verbatim (mirroring
/// `FinishReason::Other`) so a 0.1.0-pinned consumer degrades instead of failing.
/// Serde is hand-rolled (below) to route the unknown tag, not derived.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    Usage,
    ParseInput,
    Config,
    Auth,
    Provider {
        status: u16,
    },
    Transport,
    Interrupted,
    /// An unknown wire `kind`, carrying its tag verbatim for passthrough.
    Other(String),
}

/// The externally-tagged body of `ErrorKind::Provider` — `{"status": N}`, so the
/// variant renders `{"provider":{"status":N}}` exactly as the derived repr did.
#[derive(Serialize)]
struct ProviderBody {
    status: u16,
}

impl Serialize for ErrorKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ErrorKind::Usage => s.serialize_str("usage"),
            ErrorKind::ParseInput => s.serialize_str("parse_input"),
            ErrorKind::Config => s.serialize_str("config"),
            ErrorKind::Auth => s.serialize_str("auth"),
            ErrorKind::Transport => s.serialize_str("transport"),
            ErrorKind::Interrupted => s.serialize_str("interrupted"),
            ErrorKind::Provider { status } => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("provider", &ProviderBody { status: *status })?;
                m.end()
            }
            ErrorKind::Other(tag) => s.serialize_str(tag),
        }
    }
}

impl<'de> Deserialize<'de> for ErrorKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        // The tag is the string itself (unit variants) or the one object key
        // (`Provider`); any unrecognized tag rides `Other` instead of erroring.
        let tag = match &v {
            Value::String(s) => s.as_str(),
            other => other
                .as_object()
                .and_then(|m| m.keys().next())
                .map_or("", String::as_str),
        };
        Ok(match tag {
            "usage" => ErrorKind::Usage,
            "parse_input" => ErrorKind::ParseInput,
            "config" => ErrorKind::Config,
            "auth" => ErrorKind::Auth,
            "transport" => ErrorKind::Transport,
            "interrupted" => ErrorKind::Interrupted,
            "provider" => ErrorKind::Provider {
                status: v["provider"]["status"].as_u64().unwrap_or_default() as u16,
            },
            _ => ErrorKind::Other(tag.to_owned()),
        })
    }
}

impl ErrorKind {
    /// The §8 HTTP-status → kind table: a provider-returned 401/403 is an auth
    /// failure; every other status rides `Provider{status}`, which already computes
    /// the exit (4xx→69, 5xx→70) and `retryable` from the number — so the status,
    /// once known, needs no second table. This is the single home for "what a
    /// non-2xx provider status means," shared by every protocol's HTTP-error path.
    /// Only an error with NO governing status (a mid-stream event on a 2xx stream)
    /// derives its kind from the body instead — there is no status to read there.
    pub fn from_http_status(status: u16) -> ErrorKind {
        match status {
            401 | 403 => ErrorKind::Auth,
            _ => ErrorKind::Provider { status },
        }
    }
}

impl CanonicalError {
    /// `retryable` is a QUERY over `kind`, never a stored field that could
    /// drift: transport faults and 429/5xx provider errors are retryable.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::Transport)
            || matches!(self.kind, ErrorKind::Provider { status } if status == 429 || status >= 500)
    }

    /// The POSIX exit code for this error, computed from `kind` (§8 table).
    pub fn exit_code(&self) -> u8 {
        ExitClass::from_kind(self.kind.clone()).code()
    }
}

/// The sysexits classes (§8). The numeric `code()` is the single source of
/// truth for the exit code; the `bz` shim materializes a `process::ExitCode`
/// from it at the process boundary (kept out of the lib so the table is a pure,
/// directly-asserted `u8` rather than the opaque, untestable `ExitCode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitClass {
    Ok,
    Usage,
    NoInput,
    Unavailable,
    Software,
    NoPerm,
    Config,
    Sig(i32),
}

impl ExitClass {
    /// The numeric POSIX exit code for this class (§8 table). A signal class
    /// carries its own `128 + signo` value.
    pub fn code(self) -> u8 {
        match self {
            ExitClass::Ok => 0,
            ExitClass::Usage => 64,
            ExitClass::NoInput => 66,
            ExitClass::Unavailable => 69,
            ExitClass::Software => 70,
            ExitClass::NoPerm => 77,
            ExitClass::Config => 78,
            ExitClass::Sig(n) => n as u8,
        }
    }

    /// Pure `kind` → class table (§8): 4xx incl. 429 → `Unavailable` (69),
    /// 5xx → `Software` (70). An `Interrupted` kind defaults to the SIGINT
    /// exit; a live signal supersedes it via the signal path. An `Other`
    /// (unrecognized) kind is an unclassified software fault → `Software` (70),
    /// and is non-retryable (`retryable` matches no `Other`): we never auto-retry
    /// an error we cannot classify.
    pub fn from_kind(kind: ErrorKind) -> ExitClass {
        match kind {
            ErrorKind::Usage | ErrorKind::ParseInput => ExitClass::Usage,
            ErrorKind::Config => ExitClass::Config,
            ErrorKind::Auth => ExitClass::NoPerm,
            ErrorKind::Provider { status } if status >= 500 => ExitClass::Software,
            ErrorKind::Provider { .. } => ExitClass::Unavailable,
            ErrorKind::Transport => ExitClass::Unavailable,
            ErrorKind::Interrupted => ExitClass::Sig(130),
            ErrorKind::Other(_) => ExitClass::Software,
        }
    }

    /// Pure `io::Error` → class (§8): a broken pipe maps to the SIGPIPE exit
    /// (141), everything else to `Unavailable` (69).
    pub fn from_io(e: &io::Error) -> ExitClass {
        match e.kind() {
            io::ErrorKind::BrokenPipe => ExitClass::Sig(141),
            _ => ExitClass::Unavailable,
        }
    }
}
