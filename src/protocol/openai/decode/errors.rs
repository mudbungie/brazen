//! The mid-stream error projection of the OpenAI chat stream (§4.3): a
//! `data: {"error":…}` frame on a 2xx stream has no governing HTTP status, so its
//! `kind` decodes from the BODY (CR-10). The Chat Completions dialect is served by ONE
//! row across a heterogeneous class (OpenAI, Azure, OpenRouter, LiteLLM, vLLM, Mistral),
//! so the discriminator varies: a numeric `code` IS an HTTP status (the OpenRouter/proxy
//! convention — decoded through the shared table, the Google precedent §4.8), else the
//! string `type`/`code` is bucketed like the anthropic mid-stream table. The whole-body
//! HTTP case lives in the shared `json::http_error` — status is authoritative there.
//! `super::decode` dispatches into these.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::json::text_of;

/// A mid-stream `{"error":…}` frame on a 2xx stream (§4.3): `kind` decodes from the
/// BODY (CR-10 — a 2xx stream has no governing status), `error.message` → `message`,
/// the whole `error` object → `provider_detail` verbatim. `retry_after_seconds` is
/// inherently `None` (no governing header on a mid-stream 2xx error, bl-135a). NEVER
/// folded into `Finish`, and it does NOT set `state.terminated` — an error is not a
/// clean terminal marker (the arch §5.6 marker set excludes it), so a bare EOF that
/// follows still fires the premature-EOF Transport, last-error-wins (§4.3).
pub(super) fn stream_error(err: &Value) -> CanonicalError {
    CanonicalError {
        kind: error_kind(err),
        message: text_of(err, "message"),
        provider_detail: Some(err.clone()),
        retry_after_seconds: None,
    }
}

/// Mid-stream `error` object → `ErrorKind` (§4.3) — the in-band case only, where no
/// HTTP status governs. A numeric `code` IS an HTTP status (OpenRouter/LiteLLM/proxy)
/// and decodes through the shared `from_http_status` table; else the string `type`
/// (falling back to a string `code`) buckets like the anthropic mid-stream table:
/// rate-limit-ish → `Provider{429}`, server/overloaded-ish → `Provider{500}`, else
/// retryable `Transport` — the honest read of a kindless / client-error body.
fn error_kind(err: &Value) -> ErrorKind {
    if let Some(status) = err["code"].as_u64() {
        return ErrorKind::from_http_status(status as u16);
    }
    let tag = err["type"]
        .as_str()
        .or_else(|| err["code"].as_str())
        .unwrap_or_default();
    if tag.contains("rate_limit") || tag.contains("quota") {
        ErrorKind::Provider { status: 429 }
    } else if tag.contains("server") || tag.contains("overload") || tag.contains("unavailable") {
        ErrorKind::Provider { status: 500 }
    } else {
        ErrorKind::Transport // safe default: retryable, exit 69
    }
}
