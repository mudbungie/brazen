//! Canonical error model (§3.3, §8): the closed `kind` taxonomy, the computed
//! `retryable` query, and the pure exit-code tables. No IO; every mapping is a
//! pure function of `kind`/`io::Error` so it is table-tested without a network.

use std::io;

use serde::{Deserialize, Serialize};
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

/// The closed taxonomy every failure normalizes to (§3.3). `Provider` carries
/// the HTTP status so `retryable`/exit can be derived without a second table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Usage,
    ParseInput,
    Config,
    Auth,
    Provider { status: u16 },
    Transport,
    Interrupted,
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
        ExitClass::from_kind(self.kind).code()
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
    /// exit; a live signal supersedes it via the signal path.
    pub fn from_kind(kind: ErrorKind) -> ExitClass {
        match kind {
            ErrorKind::Usage | ErrorKind::ParseInput => ExitClass::Usage,
            ErrorKind::Config => ExitClass::Config,
            ErrorKind::Auth => ExitClass::NoPerm,
            ErrorKind::Provider { status } if status >= 500 => ExitClass::Software,
            ErrorKind::Provider { .. } => ExitClass::Unavailable,
            ErrorKind::Transport => ExitClass::Unavailable,
            ErrorKind::Interrupted => ExitClass::Sig(130),
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
