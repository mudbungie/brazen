//! The mid-stream error projection of the Ollama stream (§5.9): an `{"error":…}`
//! line on a 2xx stream carries a BARE STRING with no type/code discriminator, so
//! its decoded kind is retryable `Transport` (the honest read of a kindless body,
//! CR-10). The whole-body HTTP case lives in the shared `json::http_error` — status
//! is authoritative there. `super::decode` dispatches into these.

use crate::canonical::{CanonicalError, ErrorKind};

/// A mid-stream `{"error":…}` line on a 2xx stream (§5.9): `kind` decodes from the
/// body (CR-10, never the transport — a 2xx stream has none), but Ollama's envelope
/// is a BARE STRING with no `type`/`code` discriminator, so the decoded kind is
/// retryable `Transport` (exit 69) — the honest read of a kindless body, not an
/// un-decoded default. Never folded into `Finish`.
pub(super) fn stream_error(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
